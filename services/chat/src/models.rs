use serde::{Deserialize, Serialize};

use crate::attachment::Attachment;

/// Message as returned by API / sent via WebSocket.
///
/// Snowflake ID fields (`hub_id`, `channel_id`, `message_id`, `thread_root_id`)
/// are wired as strings because they exceed `Number.MAX_SAFE_INTEGER`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub hub_id: i64,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub channel_id: i64,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub message_id: i64,
    /// Already a string (stringified Snowflake of users.id or temp_users.id).
    pub author_id: String,
    pub author_type: String,
    pub content: String,
    #[serde(with = "matehub_common::serde_i64::option_as_string", default)]
    pub thread_root_id: Option<i64>,
    pub mentions: Vec<String>,
    pub mention_groups: Vec<String>,
    pub mention_everyone: bool,
    /// Structured media attachments (parsed from stored JSON).
    pub attachments: Vec<Attachment>,
    pub edited_at: Option<String>,
    pub deleted_at: Option<String>,
    pub client_id: Option<String>,
    pub bucket: i32,
}

/// Request to send a message
#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
    pub client_id: Option<String>, // UUIDv7 from client for idempotency
    #[serde(with = "matehub_common::serde_i64::option_as_string", default)]
    pub thread_root_id: Option<i64>,
    /// Attachment IDs or full Attachment objects (both accepted).
    #[serde(default)]
    pub attachments: Option<Vec<Attachment>>,
}

/// WS gateway opcodes (Discord-inspired)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Opcode {
    Dispatch = 0,
    Heartbeat = 1,
    Identify = 2,
    Resume = 6,
    Reconnect = 7,
    InvalidSession = 9,
    Hello = 10,
    HeartbeatAck = 11,
}

/// WS frame from server
#[derive(Debug, Serialize)]
pub struct GatewayEvent {
    pub op: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub s: Option<u64>, // sequence number (only for DISPATCH)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub t: Option<String>, // event type (only for DISPATCH)
    pub d: serde_json::Value, // payload
}

/// WS frame from client
#[derive(Debug, Deserialize)]
pub struct GatewayCommand {
    pub op: u8,
    pub d: serde_json::Value,
}

/// Event types for DISPATCH
pub mod events {
    pub const MESSAGE_CREATE: &str = "MESSAGE_CREATE";
    pub const MESSAGE_UPDATE: &str = "MESSAGE_UPDATE";
    pub const MESSAGE_DELETE: &str = "MESSAGE_DELETE";
    pub const ATTACHMENT_UPDATED: &str = "ATTACHMENT_UPDATED";
    pub const TYPING_START: &str = "TYPING_START";
    pub const PRESENCE_UPDATE: &str = "PRESENCE_UPDATE";
}
