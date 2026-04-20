use std::collections::HashMap;

use std::sync::Arc;

use chrono::{DateTime, Utc};
use crossbeam::channel::Sender;
use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::sfu::SfuCommand;
use crate::signaling::ServerMessage;

pub type SessionId = Uuid;
pub type ChannelId = Uuid;
pub type ParticipantId = Uuid;
pub type HubId = Uuid;

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<Mutex<AppStateInner>>,
    /// Channel to send commands to the SFU engine (on its dedicated thread).
    /// crossbeam Sender is Clone + Send + Sync — safe to share across WS tasks
    /// and to call `.send()` without awaiting.
    pub sfu_cmd_tx: Sender<SfuCommand>,
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
    pub fn new(sfu_cmd_tx: Sender<SfuCommand>, nats: Option<async_nats::Client>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(AppStateInner {
                sessions: HashMap::new(),
                channel_to_session: HashMap::new(),
            })),
            sfu_cmd_tx,
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
    pub ws_tx: mpsc::UnboundedSender<ServerMessage>,
    pub video_muted: bool,
    pub audio_muted: bool,
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
