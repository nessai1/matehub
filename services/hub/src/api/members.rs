use axum::{Json, Router, extract::{Path, State}, http::StatusCode, routing::{delete, get}};
use matehub_common::perms::{bits, resolve_user_perms};
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
        .route("/hubs/{hub_id}/pending-invites", get(get_pending_invites))
        .route(
            "/hubs/{hub_id}/pending-invites/permanent/{id}",
            delete(delete_pending_permanent),
        )
        .route(
            "/hubs/{hub_id}/pending-invites/temp/{id}",
            delete(delete_pending_temp),
        )
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
    user_type: String,
    expires_at: Option<DateTime<Utc>>,
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
    //
    // Filter rules:
    //   * deleted_at IS NULL          → don't list scrubbed accounts
    //   * (expires_at IS NULL         → permanent users always pass
    //      OR expires_at > now())     → temp users only while their link is alive
    let members_fut = sqlx::query_as::<_, MemberWithGroupsRow>(
        "SELECT u.id AS user_id, u.username, u.display_name, u.avatar_url,
                u.user_type, u.expires_at,
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
           AND u.deleted_at IS NULL
           AND (u.expires_at IS NULL OR u.expires_at > now())
         GROUP BY u.id, u.username, u.display_name, u.avatar_url,
                  u.user_type, u.expires_at,
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
            user_type: m.user_type,
            expires_at: m.expires_at,
        })
        .collect();

    result.sort_by(|a, b| {
        b.is_online
            .cmp(&a.is_online)
            .then(a.display_name.cmp(&b.display_name))
    });

    Ok(Json(result))
}

// ── Pending invites ────────────────────────────────
//
// Two stores feed this:
//   * `invitations` — permanent-account links not yet accepted (used_at NULL)
//   * `temp_users`  — guest links not yet walked through (active_session NULL)
// Merged client-side as one "people we're waiting on" list.

#[derive(Serialize)]
struct PendingInvite {
    /// "permanent" | "temp" — drives the icon/tooltip on the FE.
    kind: &'static str,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    id: i64,
    /// Pre-set username (permanent) or nickname (temp). All we know about
    /// the invitee until they actually show up.
    name: String,
    email: Option<String>,
    /// TTL only meaningful for temp links.
    expires_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    #[serde(with = "matehub_common::serde_i64::option_as_string", default)]
    group_id: Option<i64>,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    created_by: i64,
    /// Display name of the person who created the invite. Joined server-side
    /// so the FE can render "by Alice" without a second round-trip.
    created_by_name: String,
    created_by_username: String,
}

#[derive(sqlx::FromRow)]
struct PendingPermanentRow {
    id: i64,
    username: String,
    email: Option<String>,
    group_id: Option<i64>,
    created_at: DateTime<Utc>,
    created_by: i64,
    created_by_name: String,
    created_by_username: String,
}

#[derive(sqlx::FromRow)]
struct PendingTempRow {
    /// The temp user's `users.id` (== `temp_users.user_id`), exposed as `id`
    /// to the FE so both kinds of pending invite share the same shape.
    id: i64,
    nickname: String,
    group_id: i64,
    expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    created_by: i64,
    created_by_name: String,
    created_by_username: String,
}

async fn get_pending_invites(
    State(state): State<MembersState>,
    Path(hub_id): Path<i64>,
) -> Result<Json<Vec<PendingInvite>>, StatusCode> {
    let mut conn = crate::db::rls::hub_connection(&state.pool, hub_id)
        .await
        .map_err(|e| {
            tracing::error!(?e, %hub_id, "hub_connection failed in pending-invites");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let permanents = sqlx::query_as::<_, PendingPermanentRow>(
        "SELECT i.id, i.username, i.email, i.group_id, i.created_at,
                i.created_by,
                u.display_name AS created_by_name,
                u.username AS created_by_username
         FROM invitations i
         JOIN users u ON u.id = i.created_by
         WHERE i.hub_id = $1 AND i.used_at IS NULL",
    )
    .bind(hub_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| {
        tracing::error!(?e, %hub_id, "pending invitations SQL failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Two JOINs to users: one for the temp user themselves (nickname,
    // expires_at) and one for the inviter (creator_name).
    let temps = sqlx::query_as::<_, PendingTempRow>(
        "SELECT t.user_id AS id,
                tu.display_name AS nickname,
                t.group_id, tu.expires_at, t.created_at,
                t.created_by,
                cu.display_name AS created_by_name,
                cu.username AS created_by_username
         FROM temp_users t
         JOIN users tu ON tu.id = t.user_id
         JOIN users cu ON cu.id = t.created_by
         WHERE t.hub_id = $1
           AND t.revoked_at IS NULL
           AND tu.expires_at > now()
           AND tu.deleted_at IS NULL
           AND t.active_session IS NULL",
    )
    .bind(hub_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| {
        tracing::error!(?e, %hub_id, "pending temp_users SQL failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let mut result: Vec<PendingInvite> = Vec::with_capacity(permanents.len() + temps.len());
    for p in permanents {
        result.push(PendingInvite {
            kind: "permanent",
            id: p.id,
            name: p.username,
            email: p.email,
            expires_at: None,
            created_at: p.created_at,
            group_id: p.group_id,
            created_by: p.created_by,
            created_by_name: p.created_by_name,
            created_by_username: p.created_by_username,
        });
    }
    for t in temps {
        result.push(PendingInvite {
            kind: "temp",
            id: t.id,
            name: t.nickname,
            email: None,
            expires_at: Some(t.expires_at),
            created_at: t.created_at,
            group_id: Some(t.group_id),
            created_by: t.created_by,
            created_by_name: t.created_by_name,
            created_by_username: t.created_by_username,
        });
    }

    // Newest first — admins typically care about what they just sent out.
    result.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    Ok(Json(result))
}

// ── Delete pending ─────────────────────────────────
//
// Permission rule (same for both kinds):
//   * the inviter who created the row, OR
//   * a member who currently holds the matching create-perm
//     (INVITE_PERMANENT for permanent rows, CREATE_TEMP_LINKS for temp), OR
//   * MANAGE_MEMBERS as a catch-all admin override.
//
// The creator clause matters because perms can be revoked over time — an
// inviter who lost their CREATE_TEMP_LINKS bit should still be able to
// retract a link they sent yesterday.

async fn delete_pending_permanent(
    State(state): State<MembersState>,
    Path((hub_id, invitation_id)): Path<(i64, i64)>,
    auth: crate::auth::AuthUser,
) -> Result<StatusCode, StatusCode> {
    let mut conn = crate::db::rls::hub_connection(&state.pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Lookup-then-authorize. Pre-checking the row also lets us 404 before
    // the perm lookup, so an unrelated probe gets the cheaper error path.
    let row: Option<(i64, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT created_by, used_at FROM invitations WHERE id = $1 AND hub_id = $2",
    )
    .bind(invitation_id)
    .bind(hub_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let (created_by, used_at) = row.ok_or(StatusCode::NOT_FOUND)?;
    // Already accepted — there's a real user now, can't undo via this route.
    if used_at.is_some() {
        return Err(StatusCode::GONE);
    }

    if created_by != auth.0.sub {
        let caller = resolve_user_perms(&state.pool, hub_id, auth.0.sub)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if !caller.has(bits::INVITE_PERMANENT) && !caller.has(bits::MANAGE_MEMBERS) {
            return Err(StatusCode::FORBIDDEN);
        }
    }

    sqlx::query("DELETE FROM invitations WHERE id = $1 AND hub_id = $2")
        .bind(invitation_id)
        .bind(hub_id)
        .execute(&mut *conn)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    tracing::info!(%hub_id, %invitation_id, caller = auth.0.sub, "pending permanent invite deleted");
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_pending_temp(
    State(state): State<MembersState>,
    Path((hub_id, user_id)): Path<(i64, i64)>,
    auth: crate::auth::AuthUser,
) -> Result<StatusCode, StatusCode> {
    let mut conn = crate::db::rls::hub_connection(&state.pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let row: Option<(i64, Option<chrono::DateTime<chrono::Utc>>, Option<String>)> = sqlx::query_as(
        "SELECT created_by, revoked_at, active_session
         FROM temp_users WHERE user_id = $1 AND hub_id = $2",
    )
    .bind(user_id)
    .bind(hub_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let (created_by, revoked_at, active_session) = row.ok_or(StatusCode::NOT_FOUND)?;
    // Already revoked, or already walked through (we only show "pending"
    // ones in the sidebar; revoking an active session is a different flow).
    if revoked_at.is_some() || active_session.is_some() {
        return Err(StatusCode::GONE);
    }

    if created_by != auth.0.sub {
        let caller = resolve_user_perms(&state.pool, hub_id, auth.0.sub)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if !caller.has(bits::CREATE_TEMP_LINKS) && !caller.has(bits::MANAGE_MEMBERS) {
            return Err(StatusCode::FORBIDDEN);
        }
    }

    // Soft-revoke: mark temp_users.revoked_at AND slam users.expires_at to
    // now() so a future GET /join/{token} can't promote this user. The synth
    // users row stays for audit/history (no one's referenced it yet, but the
    // shape stays consistent with the kicked-after-walkthrough case).
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    sqlx::query(&format!("SET LOCAL app.current_hub_id = '{hub_id}'"))
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    sqlx::query(
        "UPDATE temp_users SET revoked_at = now(), active_session = NULL
         WHERE user_id = $1 AND hub_id = $2",
    )
    .bind(user_id)
    .bind(hub_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    sqlx::query("UPDATE users SET expires_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    tx.commit()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    tracing::info!(%hub_id, %user_id, caller = auth.0.sub, "pending temp invite revoked");
    Ok(StatusCode::NO_CONTENT)
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
