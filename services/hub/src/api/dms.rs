//! Direct Message channels.
//!
//! DMs live inside a hub but bypass the bit-permission model: participation
//! alone grants R/W. The two endpoints here are the only legal way to mint a
//! `type='dm'` channel — the generic `POST /channels` rejects that type so we
//! always go through the get-or-create path with the `dm_pair_key` invariant.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use matehub_common::snowflake;
use serde::{Deserialize, Serialize};
use sqlx::{Acquire, PgPool};

use crate::auth::AuthUser;
use crate::db::rls::hub_connection;
use crate::models::Channel;

pub fn routes(pool: PgPool) -> Router {
    Router::new()
        .route("/hubs/{hub_id}/dms", get(list_dms).post(open_dm))
        .with_state(pool)
}

// ── Helpers ─────────────────────────────────────────

/// Stable, order-independent identifier for a 1:1 DM. The partial UNIQUE index
/// on `(hub_id, dm_pair_key)` uses this as its lookup key, so two `open_dm`
/// calls in either order land on the same channel.
pub fn dm_pair_key(a: i64, b: i64) -> String {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    format!("{lo}:{hi}")
}

// ── DTOs ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct OpenDmRequest {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    recipient_id: i64,
}

#[derive(Debug, Serialize)]
struct DmChannelResponse {
    #[serde(flatten)]
    channel: Channel,
    #[serde(with = "matehub_common::serde_i64::vec_as_string")]
    participants: Vec<i64>,
}

// ── POST /v1/hubs/{hub_id}/dms ──────────────────────

async fn open_dm(
    State(pool): State<PgPool>,
    Path(hub_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<OpenDmRequest>,
) -> Result<(StatusCode, Json<DmChannelResponse>), StatusCode> {
    let me = auth.0.sub;
    let other = body.recipient_id;

    if me == other {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Both sides must actually be in this hub. Without this check a caller
    // could DM users from another hub by guessing user_ids.
    let both_members: Option<bool> = sqlx::query_scalar(
        "SELECT (
            (SELECT COUNT(*) FROM hub_members WHERE hub_id = $1 AND user_id IN ($2, $3)) = 2
        )",
    )
    .bind(hub_id)
    .bind(me)
    .bind(other)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if !both_members.unwrap_or(false) {
        return Err(StatusCode::NOT_FOUND);
    }

    let pair_key = dm_pair_key(me, other);

    // Idempotent create: try to insert with a fresh snowflake; on conflict the
    // partial UNIQUE index drops the row and we read the pre-existing one back.
    let mut tx = conn
        .begin()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    sqlx::query(
        "INSERT INTO channels (id, hub_id, name, type, position, dm_pair_key)
         VALUES ($1, $2, '', 'dm', 0, $3)
         ON CONFLICT (hub_id, dm_pair_key) WHERE dm_pair_key IS NOT NULL DO NOTHING",
    )
    .bind(snowflake::next_id())
    .bind(hub_id)
    .bind(&pair_key)
    .execute(&mut *tx)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let channel = sqlx::query_as::<_, Channel>(
        "SELECT * FROM channels WHERE hub_id = $1 AND dm_pair_key = $2",
    )
    .bind(hub_id)
    .bind(&pair_key)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    sqlx::query(
        "INSERT INTO dm_participants (channel_id, user_id) VALUES ($1, $2), ($1, $3)
         ON CONFLICT DO NOTHING",
    )
    .bind(channel.id)
    .bind(me)
    .bind(other)
    .execute(&mut *tx)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    tx.commit()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // New DM channel — chat's ACL cache for it must be cold-busted so the
    // first message both participants send doesn't 403.
    crate::acl_publish::invalidate_channel(hub_id, channel.id).await;

    Ok((
        StatusCode::OK,
        Json(DmChannelResponse {
            channel,
            participants: vec![me, other],
        }),
    ))
}

// ── GET /v1/hubs/{hub_id}/dms ───────────────────────

async fn list_dms(
    State(pool): State<PgPool>,
    Path(hub_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<DmChannelResponse>>, StatusCode> {
    let me = auth.0.sub;
    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let channels = sqlx::query_as::<_, Channel>(
        "SELECT c.* FROM channels c
         JOIN dm_participants p ON p.channel_id = c.id
         WHERE c.hub_id = $1 AND c.type = 'dm' AND p.user_id = $2
         ORDER BY c.created_at DESC",
    )
    .bind(hub_id)
    .bind(me)
    .fetch_all(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if channels.is_empty() {
        return Ok(Json(vec![]));
    }

    // One round-trip for all participants instead of N+1.
    let ids: Vec<i64> = channels.iter().map(|c| c.id).collect();
    #[derive(sqlx::FromRow)]
    struct PRow {
        channel_id: i64,
        user_id: i64,
    }
    let rows = sqlx::query_as::<_, PRow>(
        "SELECT channel_id, user_id FROM dm_participants WHERE channel_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut by_channel: std::collections::HashMap<i64, Vec<i64>> = std::collections::HashMap::new();
    for r in rows {
        by_channel.entry(r.channel_id).or_default().push(r.user_id);
    }

    let response = channels
        .into_iter()
        .map(|c| {
            let participants = by_channel.remove(&c.id).unwrap_or_default();
            DmChannelResponse {
                channel: c,
                participants,
            }
        })
        .collect();

    Ok(Json(response))
}

// ── Unit tests ──────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dm_pair_key_is_symmetric() {
        assert_eq!(dm_pair_key(1, 2), dm_pair_key(2, 1));
        assert_eq!(dm_pair_key(1, 2), "1:2");
    }

    #[test]
    fn dm_pair_key_handles_large_snowflakes() {
        let a = 9_223_372_036_854_775_000i64;
        let b = 1_000_000_000_000_000i64;
        assert_eq!(dm_pair_key(a, b), dm_pair_key(b, a));
        assert_eq!(dm_pair_key(a, b), format!("{b}:{a}"));
    }

    #[test]
    fn dm_pair_key_with_self_still_produces_a_value() {
        // The endpoint rejects me==other before ever calling this, but the
        // function itself shouldn't panic on equal inputs.
        assert_eq!(dm_pair_key(42, 42), "42:42");
    }
}
