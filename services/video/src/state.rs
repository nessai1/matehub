use std::collections::HashMap;

use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::sfu::SfuPool;
use crate::signaling::ServerMessage;

// Internal-only identifiers (never leave the SFU pod) stay as Uuid — they're
// safe to JSON-encode as 36-char strings and JS consumes them fine.
pub type SessionId = Uuid;
pub type ParticipantId = Uuid;

// Cluster-level identifiers come from hub's Snowflake space and exceed
// MAX_SAFE_INTEGER in JS. We keep them as i64 inside Rust but serialize as
// strings on the wire (see api/sessions.rs for request/response typing).
pub type ChannelId = i64;
pub type HubId = i64;

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<Mutex<AppStateInner>>,
    /// SFU command router. Replaces a single Sender<SfuCommand> — internally
    /// owns N shard senders and routes by session id (consistent hash on
    /// Uuid). Clone is Arc-cheap.
    pub sfu_pool: SfuPool,
    /// Optional NATS client for publishing voice-occupancy events to the hub
    /// service. `None` when NATS isn't reachable — video still works, but the
    /// sidebar roster falls back to polling.
    pub nats: Option<async_nats::Client>,
}

pub struct AppStateInner {
    pub sessions: HashMap<SessionId, Session>,
    pub channel_to_session: HashMap<ChannelId, SessionId>,
}

impl AppState {
    pub fn new(sfu_pool: SfuPool, nats: Option<async_nats::Client>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(AppStateInner {
                sessions: HashMap::new(),
                channel_to_session: HashMap::new(),
            })),
            sfu_pool,
            nats,
        }
    }
}

pub struct Session {
    pub id: SessionId,
    pub channel_id: ChannelId,
    /// Hub this session belongs to. Needed so voice-occupancy events can be
    /// routed to the right hub's presence WS clients.
    pub hub_id: HubId,
    pub participants: HashMap<ParticipantId, Participant>,
    pub created_at: DateTime<Utc>,
}

pub struct Participant {
    pub id: ParticipantId,
    pub user_id: String,
    pub state: ParticipantState,
    pub ws_tx: mpsc::Sender<ServerMessage>,
    pub video_muted: bool,
    pub audio_muted: bool,
    /// Discord-style "I can't hear the call right now". Doesn't change
    /// SFU forwarding (audio still flows to the deafened client, their
    /// browser silences it via a local GainNode), but other participants
    /// see a headphone-off icon on this user's tile and know not to
    /// expect a response. Replayed to late joiners alongside the
    /// audio/video mute states (see `ws.rs` bootstrap-replay).
    pub deafened: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticipantState {
    Connecting,
    Connected,
    #[allow(dead_code)] // will be used for graceful disconnect tracking
    Disconnected,
}

/// API response types
#[derive(Serialize)]
pub struct SessionResponse {
    pub session_id: SessionId,
    pub ws_url: String,
    pub created: bool,
}

#[derive(Serialize)]
pub struct SessionInfoResponse {
    pub session_id: SessionId,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub channel_id: ChannelId,
    pub participants: Vec<ParticipantInfoResponse>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct ParticipantInfoResponse {
    pub id: ParticipantId,
    pub user_id: String,
    pub state: ParticipantState,
}
