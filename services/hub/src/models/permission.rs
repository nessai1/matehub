use serde::{Deserialize, Serialize};

/// Permission bits -- shared vocabulary across all services.
///
/// Bits 0-6: per-channel (stored in channel_permissions.allow_bits/deny_bits)
/// Bits 7-13: hub-level (stored in groups.hub_permissions)
///
/// Video service checks CONNECT/SPEAK/VIDEO.
/// Chat service checks READ/WRITE.
/// Hub service checks everything.
pub mod bits {
    // ── Per-channel (0-6) ────────────────────────
    pub const READ: i32 = 1;         // bit 0: see channel, read messages
    pub const WRITE: i32 = 2;        // bit 1: send messages
    pub const CONNECT: i32 = 4;      // bit 2: join voice channel
    pub const SPEAK: i32 = 8;        // bit 3: unmute mic in voice
    pub const VIDEO: i32 = 16;       // bit 4: enable camera in voice
    pub const MANAGE_CHANNEL: i32 = 32;  // bit 5: edit channel settings, kick from voice
    pub const ADMIN_CHANNEL: i32 = 64;   // bit 6: delete others' messages, manage channel permissions

    // ── Hub-level (7-13) ─────────────────────────
    pub const CREATE_TEXT_CHANNELS: i32 = 128;   // bit 7
    pub const CREATE_VOICE_CHANNELS: i32 = 256;  // bit 8
    pub const EDIT_OTHER_CHANNELS: i32 = 512;    // bit 9
    pub const MANAGE_MEMBERS: i32 = 1024;        // bit 10: kick (only lower position)
    pub const INVITE_PERMANENT: i32 = 2048;      // bit 11: email invites
    pub const CREATE_TEMP_LINKS: i32 = 4096;     // bit 12: temporary invite links
    pub const MANAGE_ROLES: i32 = 8192;          // bit 13: CRUD groups below own position

    // ── Presets ──────────────────────────────────
    pub const ALL: i32 = 16383;           // bits 0-13 all set
    pub const MEMBER_CHANNEL: i32 = 31;   // READ | WRITE | CONNECT | SPEAK | VIDEO
    pub const MEMBER_HUB: i32 = 0;        // no hub-level perms by default
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChannelPermission {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub channel_id: i64,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub group_id: i64,
    pub allow_bits: i32,
    pub deny_bits: i32,
}

#[derive(Debug, Deserialize)]
pub struct SetPermission {
    pub allow_bits: i32,
    pub deny_bits: i32,
}

/// Computed effective permission for a user on a channel.
/// Union of all group allows, then subtract any denies.
pub fn effective_permissions(perms: &[ChannelPermission]) -> i32 {
    let allow = perms.iter().fold(0, |acc, p| acc | p.allow_bits);
    let deny = perms.iter().fold(0, |acc, p| acc | p.deny_bits);
    allow & !deny
}

pub fn has_permission(effective: i32, bit: i32) -> bool {
    effective & bit == bit
}
