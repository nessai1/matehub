use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use axum_extra::extract::Multipart;
use matehub_common::perms::resolve_user_perms;
use serde::{Deserialize, Serialize};
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
        .route("/hubs/{hub_id}", get(get_hub).patch(patch_hub))
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

#[derive(Deserialize)]
struct PatchHub {
    name: Option<String>,
    description: Option<String>,
}

// Admin-only. Slug, plan, avatar_url are *not* writable here:
//   - slug: changing it invalidates outstanding invite links → separate flow
//   - plan: billing-side concern
//   - avatar_url: dedicated multipart upload endpoint above
async fn patch_hub(
    State(state): State<HubsState>,
    Path(hub_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<PatchHub>,
) -> Result<Json<Hub>, StatusCode> {
    if auth.0.hub_id != hub_id {
        return Err(StatusCode::FORBIDDEN);
    }

    let caller = resolve_user_perms(&state.pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !caller.is_admin {
        return Err(StatusCode::FORBIDDEN);
    }

    if let Some(name) = body.name.as_deref() {
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.chars().count() > 80 {
            return Err(StatusCode::BAD_REQUEST);
        }
    }
    if let Some(desc) = body.description.as_deref()
        && desc.chars().count() > 500
    {
        return Err(StatusCode::BAD_REQUEST);
    }

    // COALESCE($1, name) keeps the existing value when the field is null in
    // the request, so PATCH semantics are partial-update without forcing the
    // client to send the full row back.
    let hub = sqlx::query_as::<_, Hub>(
        "UPDATE hubs
            SET name = COALESCE($2, name),
                description = COALESCE($3, description),
                updated_at = now()
          WHERE id = $1
          RETURNING *",
    )
    .bind(hub_id)
    .bind(body.name.as_ref().map(|s| s.trim()))
    .bind(body.description.as_deref())
    .fetch_optional(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    tracing::info!(%hub_id, "hub settings updated");
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
// Admin-only. Same gate as PATCH /v1/hubs/{id} so the General Settings
// dialog works as one operation regardless of whether the caller happens
// to be the original creator.

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
    let storage = state
        .storage
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;

    if auth.0.hub_id != hub_id {
        return Err(StatusCode::FORBIDDEN);
    }

    let caller = resolve_user_perms(&state.pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !caller.is_admin {
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
