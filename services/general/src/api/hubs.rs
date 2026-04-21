use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, post},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::extract::Authed;
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/hubs", post(create_hub))
        .route("/api/hubs/{slug}", delete(delete_hub))
        .route("/api/hubs/{slug}/token", post(exchange_for_hub_token))
}

#[derive(Deserialize)]
struct CreateHubRequest {
    slug: String,
    name: String,
}

#[derive(Serialize)]
struct CreateHubResponse {
    id: Uuid,
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

    let hub_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO hubs (slug, name, owner_account_id, status)
        VALUES ($1, $2, $3, 'provisioning')
        RETURNING id
        "#,
    )
    .bind(&req.slug)
    .bind(&req.name)
    .bind(claims.sub)
    .fetch_one(&mut *tx)
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
        tokio::spawn(async move {
            match k8s.provision_hub(&slug, hub_id).await {
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
        tracing::warn!("k8s client not configured — hub stays in 'provisioning' forever");
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

#[derive(Serialize)]
struct HubTokenResponse {
    /// One-shot token the frontend puts into the redirect URL to the hub
    /// subdomain. Hub service exchanges it for a hub-scoped session cookie.
    handoff_token: String,
    hub_url: String,
}

async fn exchange_for_hub_token(
    State(_state): State<Arc<AppState>>,
    Authed(_claims): Authed,
    Path(_slug): Path<String>,
) -> Result<Json<HubTokenResponse>, (StatusCode, String)> {
    // TODO: sign a short-lived (60s) handoff token with hub_id + account_id,
    // return https://<slug>.matehub.io/auth/handoff?token=...
    Ok(Json(HubTokenResponse {
        handoff_token: "TODO".into(),
        hub_url: "TODO".into(),
    }))
}
