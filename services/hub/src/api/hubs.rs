use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use axum_extra::extract::Multipart;
use serde::Serialize;
use sqlx::PgPool;

use crate::auth::AuthUser;
use crate::models::Hub;
use crate::storage::S3Storage;

#[derive(Clone)]
pub struct HubsState {
    pub pool: PgPool,
    pub storage: Option<Arc<S3Storage>>,
}

pub fn routes(state: HubsState) -> Router {
    Router::new()
        .route("/hubs/{hub_id}", get(get_hub))
        .route("/hubs/{hub_id}/members", get(get_members))
        .route("/hubs/{hub_id}/avatar", post(upload_hub_avatar))
        .with_state(state)
}

async fn get_hub(
    State(state): State<HubsState>,
    Path(hub_id): Path<i64>,
) -> Result<Json<Hub>, StatusCode> {
    let hub = sqlx::query_as::<_, Hub>("SELECT * FROM hubs WHERE id = $1")
        .bind(hub_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(hub))
}

#[derive(Serialize, sqlx::FromRow)]
struct MemberRow {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    user_id: i64,
    username: String,
    display_name: String,
    avatar_url: Option<String>,
    role: String,
}

async fn get_members(
    State(state): State<HubsState>,
    Path(hub_id): Path<i64>,
) -> Result<Json<Vec<MemberRow>>, StatusCode> {
    let members = sqlx::query_as::<_, MemberRow>(
        "SELECT u.id AS user_id, u.username, u.display_name, u.avatar_url, hm.role
         FROM users u
         JOIN hub_members hm ON hm.user_id = u.id
         WHERE hm.hub_id = $1
         ORDER BY hm.joined_at",
    )
    .bind(hub_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(members))
}

// ── Hub icon upload ─────────────────────────────────────────────
// Creator-only. No dedicated permission bit because this is a single-
// owner operation in boxed (the wizard's step 2 calls it once); SaaS
// can layer a richer ACL on top later.

#[derive(Serialize)]
struct UploadResponse {
    url: String,
}

async fn upload_hub_avatar(
    State(state): State<HubsState>,
    Path(hub_id): Path<i64>,
    auth: AuthUser,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, StatusCode> {
    let storage = state.storage.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;

    if auth.0.hub_id != hub_id {
        return Err(StatusCode::FORBIDDEN);
    }

    let creator_id: Option<i64> =
        sqlx::query_scalar("SELECT creator_id FROM hubs WHERE id = $1")
            .bind(hub_id)
            .fetch_one(&state.pool)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if creator_id != Some(auth.0.sub) {
        return Err(StatusCode::FORBIDDEN);
    }

    let field = multipart
        .next_field()
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?
        .ok_or(StatusCode::BAD_REQUEST)?;

    let content_type = field.content_type().unwrap_or("image/png").to_string();
    if !content_type.starts_with("image/") {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    let data = field.bytes().await.map_err(|_| StatusCode::BAD_REQUEST)?;
    if data.len() > 2 * 1024 * 1024 {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }

    let ext = match content_type.as_str() {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "png",
    };

    let rand_suffix: u64 = rand::random();
    let key = format!("{hub_id}/icon_{rand_suffix:016x}.{ext}");

    let url = storage
        .upload(&key, &content_type, data.to_vec())
        .await
        .map_err(|e| {
            tracing::error!("S3 upload failed: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    sqlx::query("UPDATE hubs SET avatar_url = $1, updated_at = now() WHERE id = $2")
        .bind(&url)
        .bind(hub_id)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    tracing::info!(%hub_id, %key, "hub avatar uploaded");

    Ok(Json(UploadResponse { url }))
}
