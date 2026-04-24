use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, post},
};
use matehub_common::snowflake;
use rand::{Rng, rng};
use serde::{Deserialize, Serialize};

use crate::api::extract::Authed;
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/hubs", post(create_hub))
        .route("/api/hubs/{slug}", delete(delete_hub))
}

/// 32 bytes of randomness, lowercase-hex. Doubles as the hub's JWT-signing
/// secret and as Bearer for /internal/auth/exchange.
fn generate_hub_secret() -> String {
    let bytes: [u8; 32] = rng().random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Deserialize)]
struct CreateHubRequest {
    slug: String,
    name: String,
}

#[derive(Serialize)]
struct CreateHubResponse {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    id: i64,
    slug: String,
    name: String,
    status: String,
}

async fn create_hub(
    State(state): State<Arc<AppState>>,
    Authed(claims): Authed,
    Json(req): Json<CreateHubRequest>,
) -> Result<Json<CreateHubResponse>, (StatusCode, String)> {
    // TODO: validate slug (lowercase, alnum+dash, length), reserved names, quota
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let hub_id = snowflake::next_id();
    let hub_secret = generate_hub_secret();

    sqlx::query(
        r#"
        INSERT INTO hubs (id, slug, name, owner_account_id, status, hub_secret)
        VALUES ($1, $2, $3, $4, 'provisioning', $5)
        "#,
    )
    .bind(hub_id)
    .bind(&req.slug)
    .bind(&req.name)
    .bind(claims.sub)
    .bind(&hub_secret)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        if let Some(db_err) = e.as_database_error() {
            if db_err.is_unique_violation() {
                return (StatusCode::CONFLICT, "slug taken".into());
            }
        }
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    sqlx::query(
        r#"
        INSERT INTO hub_members (hub_id, account_id, role)
        VALUES ($1, $2, 'owner')
        "#,
    )
    .bind(hub_id)
    .bind(claims.sub)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Fire off provisioning asynchronously. Writes hubs.status = 'ready' on
    // success, 'failed' on error. Frontend polls or subscribes to a status
    // SSE endpoint (TODO).
    if let Some(k8s) = state.k8s.clone() {
        let slug = req.slug.clone();
        let pool = state.pool.clone();
        // K8s needs the secret as a Secret resource ENV-injected into the
        // pod. Pass by value; the hub pod never calls back to general to
        // fetch it — cluster creation writes it once and that's it.
        let secret_for_pod = hub_secret.clone();
        tokio::spawn(async move {
            match k8s.provision_hub(&slug, hub_id, &secret_for_pod).await {
                Ok(()) => {
                    let _ = sqlx::query("UPDATE hubs SET status = 'ready' WHERE id = $1")
                        .bind(hub_id)
                        .execute(&pool)
                        .await;
                }
                Err(e) => {
                    tracing::error!(%hub_id, error = %e, "hub provisioning failed");
                    let _ = sqlx::query("UPDATE hubs SET status = 'failed' WHERE id = $1")
                        .bind(hub_id)
                        .execute(&pool)
                        .await;
                }
            }
        });
    } else {
        tracing::warn!(
            %hub_id,
            hub_secret = %hub_secret,
            "k8s client not configured — hub stays in 'provisioning'. \
             For manual dev bring-up, start the hub pod with JWT_SECRET set \
             to the hub_secret logged above."
        );
    }

    Ok(Json(CreateHubResponse {
        id: hub_id,
        slug: req.slug,
        name: req.name,
        status: "provisioning".into(),
    }))
}

async fn delete_hub(
    State(_state): State<Arc<AppState>>,
    Authed(_claims): Authed,
    Path(_slug): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    // TODO: verify caller is owner, schedule k8s teardown, mark hubs.status='deleting'
    Ok(StatusCode::NOT_IMPLEMENTED)
}
