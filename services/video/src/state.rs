use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::sfu::SfuCommand;
use crate::signaling::ServerMessage;

pub type SessionId = Uuid;
pub type ChannelId = Uuid;
pub type ParticipantId = Uuid;

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<Mutex<AppStateInner>>,
    /// Channel to send commands to the SFU engine
    pub sfu_cmd_tx: mpsc::UnboundedSender<SfuCommand>,
}

pub struct AppStateInner {
    pub sessions: HashMap<SessionId, Session>,
    pub channel_to_session: HashMap<ChannelId, SessionId>,
}

impl AppState {
    pub fn new(sfu_cmd_tx: mpsc::UnboundedSender<SfuCommand>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(AppStateInner {
                sessions: HashMap::new(),
                channel_to_session: HashMap::new(),
            })),
            sfu_cmd_tx,
        }
    }
}

pub struct Session {
    pub id: SessionId,
    pub channel_id: ChannelId,
    pub participants: HashMap<ParticipantId, Participant>,
    pub created_at: DateTime<Utc>,
}

pub struct Participant {
    pub id: ParticipantId,
    pub user_id: String,
    pub state: ParticipantState,
    pub ws_tx: mpsc::UnboundedSender<ServerMessage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticipantState {
    Connecting,
    Connected,
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
