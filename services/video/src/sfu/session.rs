use std::collections::HashMap;

use str0m::Rtc;
use str0m::change::SdpPendingOffer;
use str0m::media::{MediaKind, Mid};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::sfu::ParticipantId;
use crate::signaling::ServerMessage;

pub type SessionId = Uuid;

pub struct SfuSession {
    pub id: SessionId,
    pub participants: HashMap<ParticipantId, SfuParticipant>,
}

impl SfuSession {
    pub fn new(id: SessionId) -> Self {
        Self {
            id,
            participants: HashMap::new(),
        }
    }
}

pub struct SfuParticipant {
    pub id: ParticipantId,
    pub user_id: String,
    pub rtc: Rtc,
    pub ws_tx: mpsc::UnboundedSender<ServerMessage>,
    /// Tracks this participant is publishing (incoming to SFU)
    pub tracks_in: Vec<TrackIn>,
    /// Tracks this participant is subscribing to (outgoing from SFU)
    pub tracks_out: Vec<TrackOut>,
    /// Pending SDP offer awaiting answer from client
    pub pending_offer: Option<SdpPendingOffer>,
}

#[derive(Debug, Clone)]
pub struct TrackIn {
    pub mid: Mid,
    pub kind: MediaKind,
}

#[derive(Debug)]
pub struct TrackOut {
    pub origin: ParticipantId,
    pub origin_mid: Mid,
    pub kind: MediaKind,
    pub state: TrackOutState,
}

impl TrackOut {
    pub fn open_mid(&self) -> Option<Mid> {
        match self.state {
            TrackOutState::Open(mid) => Some(mid),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum TrackOutState {
    /// Needs SDP renegotiation to open
    ToOpen,
    /// SDP offer sent, waiting for answer
    Negotiating(Mid),
    /// Track is open and forwarding media
    Open(Mid),
}
