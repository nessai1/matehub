//! Centralized access checks for chat-service endpoints and the WS fan-out.
//!
//! The hub service is the source of truth for permissions; chat reads the
//! same Postgres tables (channels, dm_participants, channel_permissions,
//! member_groups, groups). Results are cached in Redis with a 30s TTL — that
//! keeps per-message overhead at a single GET on the hot path.
//!
//! ## Failure modes
//!
//! - No `pg` pool configured (HUB_DATABASE_URL unset): `check` returns false
//!   for every call. Fail-closed is the only safe default — without ACLs the
//!   service would happily fan out a DM to the entire hub.
//! - PG query error: log + deny.
//! - Cache miss + PG miss: deny.

use matehub_common::perms::{
    ChannelPermission, bits, effective_permissions, has_permission, resolve_user_perms,
};
use redis::AsyncCommands;
use sqlx::PgPool;

use crate::api::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Read,
    Write,
}

impl Action {
    fn bit(self) -> i32 {
        match self {
            Self::Read => bits::READ,
            Self::Write => bits::WRITE,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Read => "r",
            Self::Write => "w",
        }
    }
}

/// 30 seconds. ACL changes (group add/remove, channel-perm edit, kick) take
/// at most this long to propagate. DM participation never changes after
/// creation, so DM cache stays correct indefinitely modulo TTL.
const ACL_TTL_SECS: u64 = 30;

fn cache_key(hub_id: i64, channel_id: i64, user_id: i64, action: Action) -> String {
    format!("acl:{hub_id}:{channel_id}:{user_id}:{}", action.label())
}

pub async fn check(
    state: &AppState,
    hub_id: i64,
    channel_id: i64,
    user_id: i64,
    action: Action,
) -> bool {
    let Some(pg) = &state.pg else {
        return false; // fail-closed
    };

    let key = cache_key(hub_id, channel_id, user_id, action);

    // Cache hit fast-path. Conn is cloned per-call — Multiplexed.
    if let Some(redis) = state.redis.as_ref() {
        let mut conn = redis.clone();
        if let Ok(Some(v)) = conn.get::<_, Option<i32>>(&key).await {
            return v == 1;
        }
    }

    let allowed = match check_uncached(pg, hub_id, channel_id, user_id, action).await {
        Ok(a) => a,
        Err(e) => {
            tracing::error!(
                error = %e,
                hub_id, channel_id, user_id,
                action = ?action,
                "ACL check error — denying",
            );
            false
        }
    };

    if let Some(redis) = state.redis.as_ref() {
        let mut conn = redis.clone();
        let _: Result<(), _> = conn
            .set_ex::<_, i32, ()>(&key, if allowed { 1 } else { 0 }, ACL_TTL_SECS)
            .await;
    }

    allowed
}

async fn check_uncached(
    pg: &PgPool,
    hub_id: i64,
    channel_id: i64,
    user_id: i64,
    action: Action,
) -> sqlx::Result<bool> {
    let channel_type: Option<String> =
        sqlx::query_scalar("SELECT type FROM channels WHERE id = $1 AND hub_id = $2")
            .bind(channel_id)
            .bind(hub_id)
            .fetch_optional(pg)
            .await?;

    let Some(channel_type) = channel_type else {
        return Ok(false);
    };

    if channel_type == "dm" {
        // DMs ignore the bit-permission system: participation = full R/W.
        let is_member: Option<i32> = sqlx::query_scalar(
            "SELECT 1 FROM dm_participants WHERE channel_id = $1 AND user_id = $2",
        )
        .bind(channel_id)
        .bind(user_id)
        .fetch_optional(pg)
        .await?;
        return Ok(is_member.is_some());
    }

    let perms = resolve_user_perms(pg, hub_id, user_id).await?;
    if perms.is_admin {
        return Ok(true);
    }

    let channel_perms = sqlx::query_as::<_, ChannelPermission>(
        "SELECT cp.channel_id, cp.group_id, cp.allow_bits, cp.deny_bits
         FROM channel_permissions cp
         JOIN member_groups mg ON mg.group_id = cp.group_id AND mg.hub_id = $1
         WHERE cp.channel_id = $2 AND mg.user_id = $3",
    )
    .bind(hub_id)
    .bind(channel_id)
    .bind(user_id)
    .fetch_all(pg)
    .await?;

    let eff = effective_permissions(&channel_perms);
    Ok(has_permission(eff, action.bit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_distinguishes_actions() {
        let r = cache_key(1, 2, 3, Action::Read);
        let w = cache_key(1, 2, 3, Action::Write);
        assert_ne!(r, w);
        assert!(r.ends_with(":r"));
        assert!(w.ends_with(":w"));
    }

    #[test]
    fn cache_key_includes_all_dimensions() {
        // Same user+channel different hub → different key (defensive: a
        // collision could leak hub_a's deny into hub_b's allow check).
        assert_ne!(
            cache_key(1, 5, 9, Action::Read),
            cache_key(2, 5, 9, Action::Read)
        );
    }

    #[test]
    fn action_bit_matches_perms_module() {
        assert_eq!(Action::Read.bit(), bits::READ);
        assert_eq!(Action::Write.bit(), bits::WRITE);
    }
}
