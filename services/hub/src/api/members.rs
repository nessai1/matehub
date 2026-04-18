use axum::{Json, Router, extract::{Path, State}, http::StatusCode, routing::{delete, get}};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::presence;

#[derive(Clone)]
pub struct MembersState {
    pub pool: PgPool,
    pub redis: Option<presence::RedisPool>,
}

pub fn routes(state: MembersState) -> Router {
    Router::new()
        .route("/hubs/{hub_id}/members-full", get(get_members_full))
        .route("/hubs/{hub_id}/members/{user_id}", delete(kick_member))
        .route("/hubs/{hub_id}/my-permissions", get(my_permissions))
        .with_state(state)
}

#[derive(Serialize)]
struct MemberResponse {
    user_id: Uuid,
    username: String,
    display_name: String,
    avatar_url: Option<String>,
    is_online: bool,
    last_seen_at: Option<DateTime<Utc>>,
    groups: Vec<GroupBadge>,
    user_type: String, // "permanent" or "temp"
    expires_at: Option<DateTime<Utc>>, // temp users only
}

#[derive(Serialize)]
struct GroupBadge {
    id: Uuid,
    name: String,
    color: Option<String>,
}

#[derive(sqlx::FromRow)]
struct MemberRow {
    user_id: Uuid,
    username: String,
    display_name: String,
    avatar_url: Option<String>,
    last_seen_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct MemberGroupRow {
    user_id: Uuid,
    group_id: Uuid,
    group_name: String,
    group_color: Option<String>,
}

async fn get_members_full(
    State(mut state): State<MembersState>,
    Path(hub_id): Path<Uuid>,
) -> Result<Json<Vec<MemberResponse>>, StatusCode> {
    let mut conn = crate::db::rls::hub_connection(&state.pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Fetch permanent members
    let members = sqlx::query_as::<_, MemberRow>(
        "SELECT u.id AS user_id, u.username, u.display_name, u.avatar_url, hm.last_seen_at
         FROM users u
         JOIN hub_members hm ON hm.user_id = u.id
         WHERE hm.hub_id = $1
         ORDER BY hm.joined_at",
    )
    .bind(hub_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Fetch all member-group assignments for this hub
    let member_groups = sqlx::query_as::<_, MemberGroupRow>(
        "SELECT mg.user_id, mg.group_id, g.name AS group_name, g.color AS group_color
         FROM member_groups mg
         JOIN groups g ON g.id = mg.group_id
         WHERE mg.hub_id = $1
         ORDER BY g.position",
    )
    .bind(hub_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Batch online check from Redis
    let user_ids: Vec<Uuid> = members.iter().map(|m| m.user_id).collect();
    let online_set = if let Some(ref mut redis) = state.redis {
        presence::get_online_set(redis, hub_id, &user_ids).await
    } else {
        std::collections::HashSet::new()
    };

    tracing::debug!(%hub_id, online_count = online_set.len(), total = user_ids.len(), "presence check");

    // Build response
    let mut result: Vec<MemberResponse> = members
        .into_iter()
        .map(|m| {
            let groups: Vec<GroupBadge> = member_groups
                .iter()
                .filter(|mg| mg.user_id == m.user_id)
                .map(|mg| GroupBadge {
                    id: mg.group_id,
                    name: mg.group_name.clone(),
                    color: mg.group_color.clone(),
                })
                .collect();

            MemberResponse {
                is_online: online_set.contains(&m.user_id),
                user_id: m.user_id,
                username: m.username,
                display_name: m.display_name,
                avatar_url: m.avatar_url,
                last_seen_at: m.last_seen_at,
                groups,
                user_type: "permanent".into(),
                expires_at: None,
            }
        })
        .collect();

    // Online first, then alphabetical
    result.sort_by(|a, b| {
        b.is_online
            .cmp(&a.is_online)
            .then(a.display_name.cmp(&b.display_name))
    });

    Ok(Json(result))
}

// ── My permissions ─────────────────────────────────

#[derive(Serialize)]
struct MyPermissionsResponse {
    hub_bits: i32,
    top_position: i32,
    is_admin: bool,
    is_creator: bool,
}

async fn my_permissions(
    State(state): State<MembersState>,
    Path(hub_id): Path<Uuid>,
    auth: crate::auth::AuthUser,
) -> Result<Json<MyPermissionsResponse>, StatusCode> {
    use crate::api::auth_check::resolve_user_perms;

    let perms = resolve_user_perms(&state.pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(MyPermissionsResponse {
        hub_bits: perms.hub_bits,
        top_position: perms.top_position,
        is_admin: perms.is_admin,
        is_creator: perms.is_creator,
    }))
}

// ── Kick member ────────────────────────────────────

async fn kick_member(
    State(state): State<MembersState>,
    Path((hub_id, user_id)): Path<(Uuid, Uuid)>,
    auth: crate::auth::AuthUser,
) -> Result<StatusCode, StatusCode> {
    use crate::api::auth_check::{resolve_user_perms, resolve_target_position};
    use crate::models::permission::bits;

    let caller = resolve_user_perms(&state.pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if !caller.has(bits::MANAGE_MEMBERS) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Can't kick yourself
    if auth.0.sub == user_id {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Can't kick hub creator
    let target_is_creator: bool = sqlx::query_scalar(
        "SELECT COALESCE(creator_id = $2, false) FROM hubs WHERE id = $1",
    )
    .bind(hub_id)
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(false);
    if target_is_creator {
        return Err(StatusCode::FORBIDDEN);
    }

    // Can only kick users with lower position (higher number)
    let target_pos = resolve_target_position(&state.pool, hub_id, user_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !caller.can_manage_position(target_pos) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Remove from all groups + hub membership
    sqlx::query("DELETE FROM member_groups WHERE hub_id = $1 AND user_id = $2")
        .bind(hub_id)
        .bind(user_id)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    sqlx::query("DELETE FROM hub_members WHERE hub_id = $1 AND user_id = $2")
        .bind(hub_id)
        .bind(user_id)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    tracing::info!(%hub_id, %user_id, caller = %auth.0.sub, "member kicked");

    Ok(StatusCode::NO_CONTENT)
}
