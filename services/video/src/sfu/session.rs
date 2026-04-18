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
    #[allow(dead_code)] // used for logging, will be used for Redis in Stage 2
    pub id: SessionId,
    pub participants: HashMap<ParticipantId, SfuParticipant>,
    /// Precomputed fan-out map for O(1) lookup on every incoming media packet.
    /// Key: (publisher, publisher's mid). Value: list of (subscriber, subscriber's mid).
    ///
    /// Updated incrementally when a TrackOut transitions to Open, and when a
    /// participant leaves. Empty entries are pruned. Lives here (not on the
    /// participants) so media forwarding doesn't have to scan per packet.
    pub forwarding_map: HashMap<(ParticipantId, Mid), Vec<(ParticipantId, Mid)>>,
}

impl SfuSession {
    pub fn new(id: SessionId) -> Self {
        Self {
            id,
            participants: HashMap::new(),
            forwarding_map: HashMap::new(),
        }
    }

    /// Drop every forwarding entry touching `gone` — as publisher (key) or
    /// subscriber (value). Called on participant leave.
    pub fn drop_from_forwarding(&mut self, gone: ParticipantId) {
        self.forwarding_map
            .retain(|&(publisher, _), _| publisher != gone);
        for targets in self.forwarding_map.values_mut() {
            targets.retain(|&(subscriber, _)| subscriber != gone);
        }
        self.forwarding_map.retain(|_, targets| !targets.is_empty());
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
    /// Last time we received ANY UDP packet from this participant (STUN/DTLS/RTP/RTCP)
    pub last_activity_at: std::time::Instant,
    /// ICE is disconnected (consent check failed)
    pub ice_disconnected: bool,
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
