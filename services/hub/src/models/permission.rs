use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Permission bits -- shared vocabulary across all services.
/// Video checks CONNECT/SPEAK/VIDEO, chat checks READ/WRITE.
pub mod bits {
    pub const READ: i32 = 1;
    pub const WRITE: i32 = 2;
    pub const CONNECT: i32 = 4;
    pub const SPEAK: i32 = 8;
    pub const VIDEO: i32 = 16;
    pub const MANAGE: i32 = 32;
    pub const ADMIN: i32 = 64;
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChannelPermission {
    pub channel_id: Uuid,
    pub group_id: Uuid,
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
