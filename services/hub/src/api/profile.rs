use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{patch, post},
};
use axum_extra::extract::Multipart;
use serde::Deserialize;
use sqlx::PgPool;

use crate::auth::AuthUser;
use crate::storage::S3Storage;

#[derive(Clone)]
pub struct ProfileState {
    pub storage: Option<Arc<S3Storage>>,
    pub pool: PgPool,
}

pub fn routes(state: ProfileState) -> Router {
    Router::new()
        .route("/hubs/{hub_id}/profile", patch(update_profile))
        .route("/hubs/{hub_id}/profile/avatar", post(upload_avatar))
        .with_state(state)
}

// ── Update display name ─────────────────────────────

#[derive(Debug, Deserialize)]
struct UpdateProfileRequest {
    display_name: Option<String>,
}

#[derive(serde::Serialize)]
struct ProfileResponse {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    user_id: i64,
    display_name: String,
    avatar_url: Option<String>,
}

async fn update_profile(
    State(state): State<ProfileState>,
    Path(hub_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<UpdateProfileRequest>,
) -> Result<Json<ProfileResponse>, StatusCode> {
    if auth.0.hub_id != hub_id {
        return Err(StatusCode::FORBIDDEN);
    }

    let user_id = auth.0.sub;

    tracing::debug!(%hub_id, %user_id, ?body, "profile update requested");

    if let Some(name) = &body.display_name {
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.len() > 64 {
            tracing::warn!(%user_id, len = trimmed.len(), "display_name invalid");
            return Err(StatusCode::BAD_REQUEST);
        }

        sqlx::query("UPDATE users SET display_name = $1 WHERE id = $2")
            .bind(trimmed)
            .bind(user_id)
            .execute(&state.pool)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        tracing::info!(%hub_id, %user_id, new_name = trimmed, "display_name updated");
    }

    #[derive(sqlx::FromRow)]
    struct Row {
        display_name: String,
        avatar_url: Option<String>,
    }

    let row = sqlx::query_as::<_, Row>("SELECT display_name, avatar_url FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(ProfileResponse {
        user_id,
        display_name: row.display_name,
        avatar_url: row.avatar_url,
    }))
}

// ── Upload avatar ───────────────────────────────────

#[derive(serde::Serialize)]
struct UploadResponse {
    url: String,
}

async fn upload_avatar(
    State(state): State<ProfileState>,
    Path(hub_id): Path<i64>,
    auth: AuthUser,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, StatusCode> {
    let storage = state.storage.as_ref().ok_or_else(|| {
        tracing::error!("S3 not configured");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    let user_id = auth.0.sub;

    if auth.0.hub_id != hub_id {
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
    let key = format!("{hub_id}/avatars/{user_id}_{rand_suffix:016x}.{ext}");

    let url = storage
        .upload(&key, &content_type, data.to_vec())
        .await
        .map_err(|e| {
            tracing::error!("S3 upload failed: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    sqlx::query("UPDATE users SET avatar_url = $1 WHERE id = $2")
        .bind(&url)
        .bind(user_id)
        .execute(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("DB update failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    tracing::info!(%hub_id, %user_id, %key, "avatar uploaded and saved");

    Ok(Json(UploadResponse { url }))
}
