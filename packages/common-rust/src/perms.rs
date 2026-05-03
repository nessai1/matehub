//! Shared permission model used by hub + chat.
//!
//! Hub owns the source of truth (Postgres tables `groups`, `member_groups`,
//! `channel_permissions`). Chat reads the same tables to enforce R/W on
//! sends and on the WS fan-out path. Keeping the bit constants and the
//! resolver in one crate is the only way to keep the two services from
//! drifting on what "READ" means.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

/// Permission bits — shared vocabulary across all services.
///
/// Bits 0-6: per-channel (stored in `channel_permissions.allow_bits/deny_bits`).
/// Bits 7-13: hub-level (stored in `groups.hub_permissions`).
///
/// Video service checks CONNECT/SPEAK/VIDEO.
/// Chat service checks READ/WRITE.
/// Hub service checks everything.
pub mod bits {
    // ── Per-channel (0-6) ────────────────────────
    pub const READ: i32 = 1;
    pub const WRITE: i32 = 2;
    pub const CONNECT: i32 = 4;
    pub const SPEAK: i32 = 8;
    pub const VIDEO: i32 = 16;
    pub const MANAGE_CHANNEL: i32 = 32;
    pub const ADMIN_CHANNEL: i32 = 64;

    // ── Hub-level (7-13) ─────────────────────────
    pub const CREATE_TEXT_CHANNELS: i32 = 128;
    pub const CREATE_VOICE_CHANNELS: i32 = 256;
    pub const EDIT_OTHER_CHANNELS: i32 = 512;
    pub const MANAGE_MEMBERS: i32 = 1024;
    pub const INVITE_PERMANENT: i32 = 2048;
    pub const CREATE_TEMP_LINKS: i32 = 4096;
    pub const MANAGE_ROLES: i32 = 8192;

    // ── Presets ──────────────────────────────────
    pub const ALL: i32 = 16383;
    pub const MEMBER_CHANNEL: i32 = 31;
    pub const MEMBER_HUB: i32 = 0;
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChannelPermission {
    #[serde(with = "crate::serde_i64::as_string")]
    pub channel_id: i64,
    #[serde(with = "crate::serde_i64::as_string")]
    pub group_id: i64,
    pub allow_bits: i32,
    pub deny_bits: i32,
}

#[derive(Debug, Deserialize)]
pub struct SetPermission {
    pub allow_bits: i32,
    pub deny_bits: i32,
}

/// Union of all group allows, minus the union of all denies.
pub fn effective_permissions(perms: &[ChannelPermission]) -> i32 {
    let allow = perms.iter().fold(0, |acc, p| acc | p.allow_bits);
    let deny = perms.iter().fold(0, |acc, p| acc | p.deny_bits);
    allow & !deny
}

pub fn has_permission(effective: i32, bit: i32) -> bool {
    effective & bit == bit
}

/// User's resolved hub-level permissions + position info.
pub struct UserPerms {
    pub hub_bits: i32,
    pub top_position: i32,
    pub is_admin: bool,
    pub is_creator: bool,
}

impl UserPerms {
    pub fn has(&self, bit: i32) -> bool {
        self.is_admin || (self.hub_bits & bit == bit)
    }

    pub fn can_manage_position(&self, target_position: i32) -> bool {
        self.is_admin || self.top_position < target_position
    }

    pub fn grantable_bits(&self) -> i32 {
        if self.is_admin {
            bits::ALL
        } else {
            self.hub_bits
        }
    }
}

pub async fn resolve_user_perms(
    pool: &PgPool,
    hub_id: i64,
    user_id: i64,
) -> Result<UserPerms, sqlx::Error> {
    #[derive(sqlx::FromRow)]
    struct Row {
        name: String,
        position: i32,
        hub_permissions: i32,
    }

    let rows = sqlx::query_as::<_, Row>(
        "SELECT g.name, g.position, g.hub_permissions
         FROM groups g
         JOIN member_groups mg ON mg.group_id = g.id AND mg.hub_id = g.hub_id
         WHERE mg.hub_id = $1 AND mg.user_id = $2",
    )
    .bind(hub_id)
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    let hub_bits = rows.iter().fold(0, |acc, r| acc | r.hub_permissions);
    let top_position = rows.iter().map(|r| r.position).min().unwrap_or(i32::MAX);
    let is_admin = rows.iter().any(|r| r.name == "admin");

    let is_creator: bool =
        sqlx::query_scalar("SELECT COALESCE(creator_id = $2, false) FROM hubs WHERE id = $1")
            .bind(hub_id)
            .bind(user_id)
            .fetch_one(pool)
            .await
            .unwrap_or(false);

    Ok(UserPerms {
        hub_bits,
        top_position,
        is_admin,
        is_creator,
    })
}

pub async fn resolve_target_position(
    pool: &PgPool,
    hub_id: i64,
    target_user_id: i64,
) -> Result<i32, sqlx::Error> {
    let pos: Option<i32> = sqlx::query_scalar(
        "SELECT MIN(g.position)
         FROM groups g
         JOIN member_groups mg ON mg.group_id = g.id AND mg.hub_id = g.hub_id
         WHERE mg.hub_id = $1 AND mg.user_id = $2",
    )
    .bind(hub_id)
    .bind(target_user_id)
    .fetch_one(pool)
    .await?;

    Ok(pos.unwrap_or(i32::MAX))
}
