use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use axum_extra::extract::Multipart;
use matehub_common::snowflake;
use sqlx::PgPool;

use crate::auth::AuthUser;
use crate::api::auth_check::resolve_user_perms;
use crate::db::rls::hub_connection;
use crate::models::Channel;
use crate::models::channel::{CreateChannel, UpdateChannel};
use crate::models::permission::bits;
use crate::storage::S3Storage;

#[derive(Clone)]
pub struct ChannelsState {
    pub pool: PgPool,
    pub storage: Option<Arc<S3Storage>>,
}

pub fn routes(state: ChannelsState) -> Router {
    Router::new()
        .route(
            "/hubs/{hub_id}/channels",
            get(list_channels).post(create_channel),
        )
        .route(
            "/hubs/{hub_id}/channels/{channel_id}",
            get(get_channel)
                .patch(update_channel)
                .delete(delete_channel),
        )
        .route(
            "/hubs/{hub_id}/channels/{channel_id}/icon",
            post(upload_icon),
        )
        .with_state(state)
}

// ── List ─────────────────────────────────────────

async fn list_channels(
    State(state): State<ChannelsState>,
    Path(hub_id): Path<i64>,
) -> Result<Json<Vec<Channel>>, StatusCode> {
    let mut conn = hub_connection(&state.pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let channels = sqlx::query_as::<_, Channel>(
        "SELECT * FROM channels WHERE hub_id = $1 ORDER BY position",
    )
    .bind(hub_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(channels))
}

// ── Get ──────────────────────────────────────────

async fn get_channel(
    State(state): State<ChannelsState>,
    Path((hub_id, channel_id)): Path<(i64, i64)>,
) -> Result<Json<Channel>, StatusCode> {
    let mut conn = hub_connection(&state.pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let channel = sqlx::query_as::<_, Channel>(
        "SELECT * FROM channels WHERE id = $1 AND hub_id = $2",
    )
    .bind(channel_id)
    .bind(hub_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(channel))
}

// ── Create ───────────────────────────────────────

async fn create_channel(
    State(state): State<ChannelsState>,
    Path(hub_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<CreateChannel>,
) -> Result<(StatusCode, Json<Channel>), StatusCode> {
    let valid_types = ["text", "voice", "stage"];
    if !valid_types.contains(&body.channel_type.as_str()) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if body.name.trim().is_empty() || body.name.len() > 100 {
        return Err(StatusCode::BAD_REQUEST);
    }

    let perms = resolve_user_perms(&state.pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let required = match body.channel_type.as_str() {
        "text" => bits::CREATE_TEXT_CHANNELS,
        _ => bits::CREATE_VOICE_CHANNELS,
    };
    if !perms.has(required) {
        return Err(StatusCode::FORBIDDEN);
    }

    let mut conn = hub_connection(&state.pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let max_pos: Option<i32> =
        sqlx::query_scalar("SELECT MAX(position) FROM channels WHERE hub_id = $1")
            .bind(hub_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let channel = sqlx::query_as::<_, Channel>(
        "INSERT INTO channels (id, hub_id, name, type, position, icon_id, icon_color)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING *",
    )
    .bind(snowflake::next_id())
    .bind(hub_id)
    .bind(body.name.trim())
    .bind(&body.channel_type)
    .bind(max_pos.unwrap_or(-1) + 1)
    .bind(&body.icon_id)
    .bind(&body.icon_color)
    .fetch_one(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((StatusCode::CREATED, Json(channel)))
}

// ── Update ───────────────────────────────────────

async fn update_channel(
    State(state): State<ChannelsState>,
    Path((hub_id, channel_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(body): Json<UpdateChannel>,
) -> Result<Json<Channel>, StatusCode> {
    let perms = resolve_user_perms(&state.pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !perms.has(bits::EDIT_OTHER_CHANNELS) {
        return Err(StatusCode::FORBIDDEN);
    }

    if let Some(ref name) = body.name {
        if name.trim().is_empty() || name.len() > 100 {
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    let mut conn = hub_connection(&state.pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let channel = sqlx::query_as::<_, Channel>(
        "UPDATE channels SET
            name = COALESCE($3, name),
            position = COALESCE($4, position),
            icon_id = COALESCE($5, icon_id),
            icon_color = COALESCE($6, icon_color),
            icon_image_url = COALESCE($7, icon_image_url)
         WHERE id = $2 AND hub_id = $1
         RETURNING *",
    )
    .bind(hub_id)
    .bind(channel_id)
    .bind(&body.name)
    .bind(body.position)
    .bind(&body.icon_id)
    .bind(&body.icon_color)
    .bind(&body.icon_image_url)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(channel))
}

// ── Delete ───────────────────────────────────────

async fn delete_channel(
    State(state): State<ChannelsState>,
    Path((hub_id, channel_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, StatusCode> {
    let perms = resolve_user_perms(&state.pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !perms.has(bits::EDIT_OTHER_CHANNELS) {
        return Err(StatusCode::FORBIDDEN);
    }
    let mut conn = hub_connection(&state.pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let result = sqlx::query("DELETE FROM channels WHERE id = $1 AND hub_id = $2")
        .bind(channel_id)
        .bind(hub_id)
        .execute(&mut *conn)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if result.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    Ok(StatusCode::NO_CONTENT)
}

// ── Icon upload ──────────────────────────────────

#[derive(serde::Serialize)]
struct UploadResponse {
    url: String,
}

async fn upload_icon(
    State(state): State<ChannelsState>,
    Path((hub_id, channel_id)): Path<(i64, i64)>,
    auth: AuthUser,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, StatusCode> {
    let perms = resolve_user_perms(&state.pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !perms.has(bits::EDIT_OTHER_CHANNELS) {
        return Err(StatusCode::FORBIDDEN);
    }

    let storage = state.storage.as_ref().ok_or_else(|| {
        tracing::error!("S3 not configured");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

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
    let key = format!("{hub_id}/channels/{channel_id}_{rand_suffix:016x}.{ext}");

    let url = storage
        .upload(&key, &content_type, data.to_vec())
        .await
        .map_err(|e| {
            tracing::error!("S3 upload failed: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let mut conn = hub_connection(&state.pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let result = sqlx::query(
        "UPDATE channels SET icon_image_url = $1 WHERE id = $2 AND hub_id = $3",
    )
    .bind(&url)
    .bind(channel_id)
    .bind(hub_id)
    .execute(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Silent-success was the original bug: S3 write happens, DB update touches
    // zero rows (e.g. channel_id mismatch), caller sees "uploaded" but icon
    // disappears on next reload. Fail loud instead.
    if result.rows_affected() == 0 {
        tracing::warn!(%hub_id, %channel_id, %key, "icon uploaded to S3 but channel row missing");
        return Err(StatusCode::NOT_FOUND);
    }

    tracing::info!(%hub_id, %channel_id, %key, "channel icon uploaded");

    Ok(Json(UploadResponse { url }))
}
