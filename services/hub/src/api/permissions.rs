use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, put},
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::api::auth_check::resolve_user_perms;
use crate::db::rls::hub_connection;
use crate::models::ChannelPermission;
use crate::models::permission::{bits, SetPermission, effective_permissions, has_permission};

pub fn routes(pool: PgPool) -> Router {
    Router::new()
        .route(
            "/hubs/{hub_id}/channels/{channel_id}/permissions",
            get(list_channel_permissions),
        )
        .route(
            "/hubs/{hub_id}/channels/{channel_id}/permissions/{group_id}",
            put(set_channel_permission).delete(delete_channel_permission),
        )
        .route(
            "/hubs/{hub_id}/channels/{channel_id}/effective/{user_id}",
            get(get_effective_permissions),
        )
        .with_state(pool)
}

async fn list_channel_permissions(
    State(pool): State<PgPool>,
    Path((hub_id, channel_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<ChannelPermission>>, StatusCode> {
    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let perms = sqlx::query_as::<_, ChannelPermission>(
        "SELECT * FROM channel_permissions WHERE channel_id = $1",
    )
    .bind(channel_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(perms))
}

async fn set_channel_permission(
    State(pool): State<PgPool>,
    Path((hub_id, channel_id, group_id)): Path<(Uuid, Uuid, Uuid)>,
    auth: AuthUser,
    Json(body): Json<SetPermission>,
) -> Result<StatusCode, StatusCode> {
    let caller = resolve_user_perms(&pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !caller.has(bits::ADMIN_CHANNEL) {
        return Err(StatusCode::FORBIDDEN);
    }

    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    sqlx::query(
        "INSERT INTO channel_permissions (channel_id, group_id, allow_bits, deny_bits)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (channel_id, group_id)
         DO UPDATE SET allow_bits = $3, deny_bits = $4",
    )
    .bind(channel_id)
    .bind(group_id)
    .bind(body.allow_bits)
    .bind(body.deny_bits)
    .execute(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::NO_CONTENT)
}

async fn delete_channel_permission(
    State(pool): State<PgPool>,
    Path((hub_id, channel_id, group_id)): Path<(Uuid, Uuid, Uuid)>,
    auth: AuthUser,
) -> Result<StatusCode, StatusCode> {
    let caller = resolve_user_perms(&pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !caller.has(bits::ADMIN_CHANNEL) {
        return Err(StatusCode::FORBIDDEN);
    }

    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    sqlx::query("DELETE FROM channel_permissions WHERE channel_id = $1 AND group_id = $2")
        .bind(channel_id)
        .bind(group_id)
        .execute(&mut *conn)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::NO_CONTENT)
}

/// Get the effective permission bits for a specific user on a channel.
#[derive(serde::Serialize)]
struct EffectiveResponse {
    bits: i32,
    read: bool,
    write: bool,
    connect: bool,
    speak: bool,
    video: bool,
    manage: bool,
    admin: bool,
}

async fn get_effective_permissions(
    State(pool): State<PgPool>,
    Path((hub_id, channel_id, user_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<EffectiveResponse>, StatusCode> {
    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let perms = sqlx::query_as::<_, ChannelPermission>(
        "SELECT cp.channel_id, cp.group_id, cp.allow_bits, cp.deny_bits
         FROM channel_permissions cp
         JOIN member_groups mg ON mg.group_id = cp.group_id AND mg.hub_id = $1
         WHERE cp.channel_id = $2 AND mg.user_id = $3",
    )
    .bind(hub_id)
    .bind(channel_id)
    .bind(user_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let eff = effective_permissions(&perms);

    Ok(Json(EffectiveResponse {
        bits: eff,
        read: has_permission(eff, bits::READ),
        write: has_permission(eff, bits::WRITE),
        connect: has_permission(eff, bits::CONNECT),
        speak: has_permission(eff, bits::SPEAK),
        video: has_permission(eff, bits::VIDEO),
        manage: has_permission(eff, bits::MANAGE_CHANNEL),
        admin: has_permission(eff, bits::ADMIN_CHANNEL),
    }))
}
