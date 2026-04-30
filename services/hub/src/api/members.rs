use axum::{Json, Router, extract::{Path, State}, http::StatusCode, routing::{delete, get}};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

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
    #[serde(with = "matehub_common::serde_i64::as_string")]
    user_id: i64,
    username: String,
    display_name: String,
    avatar_url: Option<String>,
    is_online: bool,
    last_seen_at: Option<DateTime<Utc>>,
    groups: Vec<GroupBadge>,
    user_type: String,
    expires_at: Option<DateTime<Utc>>,
    /// Voice channel the user is currently connected to (if any). Populated
    /// from Redis voice_occupancy:<hub_id>, which the video service keeps in
    /// sync via NATS.
    #[serde(with = "matehub_common::serde_i64::option_as_string", default)]
    current_voice_channel_id: Option<i64>,
}

#[derive(Serialize)]
struct GroupBadge {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    id: i64,
    name: String,
    color: Option<String>,
}

/// Row shape from the consolidated members+groups query. `groups` is a JSON
/// array aggregated server-side (one round-trip instead of two), decoded
/// directly into Vec<GroupBadgeJson> via sqlx's `json` feature.
#[derive(sqlx::FromRow)]
struct MemberWithGroupsRow {
    user_id: i64,
    username: String,
    display_name: String,
    avatar_url: Option<String>,
    last_seen_at: Option<DateTime<Utc>>,
    #[sqlx(json)]
    groups: Vec<GroupBadgeJson>,
}

#[derive(Deserialize)]
struct GroupBadgeJson {
    id: i64,
    name: String,
    color: Option<String>,
}

async fn get_members_full(
    State(state): State<MembersState>,
    Path(hub_id): Path<i64>,
) -> Result<Json<Vec<MemberResponse>>, StatusCode> {
    let mut conn = crate::db::rls::hub_connection(&state.pool, hub_id)
        .await
        .map_err(|e| {
            tracing::error!(?e, %hub_id, "hub_connection failed in members-full");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // One query: members + their groups aggregated server-side. LEFT JOIN
    // covers users with zero groups; FILTER (WHERE g.id IS NOT NULL) keeps
    // the resulting array empty rather than [{null}].
    let members_fut = sqlx::query_as::<_, MemberWithGroupsRow>(
        "SELECT u.id AS user_id, u.username, u.display_name, u.avatar_url,
                hm.last_seen_at,
                COALESCE(
                    json_agg(
                        json_build_object('id', g.id, 'name', g.name, 'color', g.color)
                        ORDER BY g.position
                    ) FILTER (WHERE g.id IS NOT NULL),
                    '[]'::json
                ) AS groups
         FROM users u
         JOIN hub_members hm ON hm.user_id = u.id
         LEFT JOIN member_groups mg ON mg.user_id = u.id AND mg.hub_id = hm.hub_id
         LEFT JOIN groups g ON g.id = mg.group_id
         WHERE hm.hub_id = $1
         GROUP BY u.id, u.username, u.display_name, u.avatar_url,
                  hm.last_seen_at, hm.joined_at
         ORDER BY hm.joined_at",
    )
    .bind(hub_id)
    .fetch_all(&mut *conn);

    // Voice snapshot doesn't depend on user_ids — kick it off in parallel.
    // Online set DOES depend on user_ids, so we await it after SQL completes.
    let voice_fut = async {
        if let Some(mut redis) = state.redis.clone() {
            presence::voice_occupancy_snapshot(&mut redis, hub_id).await
        } else {
            std::collections::HashMap::new()
        }
    };

    let (members_result, voice_map) = tokio::join!(members_fut, voice_fut);
    let members = members_result.map_err(|e| {
        tracing::error!(?e, %hub_id, "members-full SQL failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let user_ids: Vec<i64> = members.iter().map(|m| m.user_id).collect();
    let online_set = if let Some(mut redis) = state.redis.clone() {
        presence::get_online_set(&mut redis, hub_id, &user_ids).await
    } else {
        std::collections::HashSet::new()
    };

    tracing::debug!(%hub_id, online_count = online_set.len(), voice_count = voice_map.len(), total = user_ids.len(), "presence check");

    let mut result: Vec<MemberResponse> = members
        .into_iter()
        .map(|m| MemberResponse {
            is_online: online_set.contains(&m.user_id),
            current_voice_channel_id: voice_map.get(&m.user_id).copied(),
            user_id: m.user_id,
            username: m.username,
            display_name: m.display_name,
            avatar_url: m.avatar_url,
            last_seen_at: m.last_seen_at,
            groups: m
                .groups
                .into_iter()
                .map(|g| GroupBadge {
                    id: g.id,
                    name: g.name,
                    color: g.color,
                })
                .collect(),
            user_type: "permanent".into(),
            expires_at: None,
        })
        .collect();

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
    Path(hub_id): Path<i64>,
    auth: crate::auth::AuthUser,
) -> Result<Json<MyPermissionsResponse>, StatusCode> {
    use matehub_common::perms::resolve_user_perms;

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
    Path((hub_id, user_id)): Path<(i64, i64)>,
    auth: crate::auth::AuthUser,
) -> Result<StatusCode, StatusCode> {
    use matehub_common::perms::{bits, resolve_target_position, resolve_user_perms};

    let caller = resolve_user_perms(&state.pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if !caller.has(bits::MANAGE_MEMBERS) {
        return Err(StatusCode::FORBIDDEN);
    }

    if auth.0.sub == user_id {
        return Err(StatusCode::BAD_REQUEST);
    }

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

    let target_pos = resolve_target_position(&state.pool, hub_id, user_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !caller.can_manage_position(target_pos) {
        return Err(StatusCode::FORBIDDEN);
    }

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

    crate::acl_publish::invalidate_user(hub_id, user_id).await;
    crate::member_events::member_left(hub_id, user_id);

    tracing::info!(%hub_id, %user_id, caller = auth.0.sub, "member kicked");

    Ok(StatusCode::NO_CONTENT)
}
