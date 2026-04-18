use std::collections::{HashMap, VecDeque};

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
    /// Source hints for upcoming MediaAdded events, keyed by kind so audio
    /// and video can be independently queued. Populated by `publish_track`
    /// signaling BEFORE the client-initiated SDP offer arrives; drained
    /// FIFO as MediaAdded events fire for the matching kind. On miss we
    /// default to `Source::Camera` (the Join-flow case, no publish_track
    /// needed because it's implicit).
    pub pending_source_hints: HashMap<MediaKind, VecDeque<Source>>,
    /// Last time we received ANY UDP packet from this participant (STUN/DTLS/RTP/RTCP)
    pub last_activity_at: std::time::Instant,
    /// ICE is disconnected (consent check failed)
    pub ice_disconnected: bool,
}

/// Logical track source. Determines msid formatting so Chrome treats audio/
/// video of different sources as independent MediaStreams (prevents A/V sync
/// lockup on missing peers — same reason we split camera audio/video).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Microphone audio or webcam video.
    Camera,
    /// `getDisplayMedia` — screen video or system/tab audio.
    Screen,
}

impl Source {
    pub fn as_msid_tag(self) -> &'static str {
        match self {
            Source::Camera => "cam",
            Source::Screen => "screen",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TrackIn {
    pub mid: Mid,
    pub kind: MediaKind,
    /// Where this track came from on the publisher side. Default `Camera` —
    /// screen tracks are explicitly marked via a `publish_track` signal
    /// arriving before the SDP offer (§5.2 screen-share design doc).
    pub source: Source,
}

#[derive(Debug)]
pub struct TrackOut {
    pub origin: ParticipantId,
    pub origin_mid: Mid,
    pub kind: MediaKind,
    /// Propagated from the publisher's TrackIn at fan-out time — controls the
    /// msid the subscriber sees (`<origin>-cam-video` vs `<origin>-screen-video`)
    /// and lets the SDK render the right tile shape.
    pub source: Source,
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

#[cfg(test)]
mod tests {
    use super::*;

    // Mid is opaque and its only public ctor is `Mid::new()` (random). Tests
    // allocate unique Mids up front and then refer to them by local binding.
    fn pid() -> ParticipantId {
        Uuid::new_v4()
    }

    #[test]
    fn new_session_is_empty() {
        let s = SfuSession::new(Uuid::new_v4());
        assert!(s.participants.is_empty());
        assert!(s.forwarding_map.is_empty());
    }

    #[test]
    fn drop_from_forwarding_removes_publisher_keys() {
        let mut s = SfuSession::new(Uuid::new_v4());
        let gone = pid();
        let other = pid();
        let sub = pid();
        let gone_mid = Mid::new();
        let other_mid = Mid::new();
        let sub_mid_x = Mid::new();
        let sub_mid_y = Mid::new();

        s.forwarding_map
            .insert((gone, gone_mid), vec![(sub, sub_mid_x)]);
        s.forwarding_map
            .insert((other, other_mid), vec![(sub, sub_mid_y)]);

        s.drop_from_forwarding(gone);

        assert!(
            !s.forwarding_map.contains_key(&(gone, gone_mid)),
            "gone-as-publisher entry must be removed"
        );
        assert!(
            s.forwarding_map.contains_key(&(other, other_mid)),
            "unrelated publisher entry must survive"
        );
    }

    #[test]
    fn drop_from_forwarding_removes_subscriber_entries() {
        let mut s = SfuSession::new(Uuid::new_v4());
        let publisher = pid();
        let gone = pid();
        let other_sub = pid();
        let pub_mid = Mid::new();

        s.forwarding_map.insert(
            (publisher, pub_mid),
            vec![(gone, Mid::new()), (other_sub, Mid::new())],
        );

        s.drop_from_forwarding(gone);

        let entry = s
            .forwarding_map
            .get(&(publisher, pub_mid))
            .expect("publisher entry should survive");
        assert_eq!(entry.len(), 1);
        assert_eq!(entry[0].0, other_sub);
    }

    #[test]
    fn drop_from_forwarding_prunes_empty_entries() {
        let mut s = SfuSession::new(Uuid::new_v4());
        let publisher = pid();
        let gone = pid();

        s.forwarding_map
            .insert((publisher, Mid::new()), vec![(gone, Mid::new())]);

        s.drop_from_forwarding(gone);

        assert!(
            s.forwarding_map.is_empty(),
            "entry with empty subscriber list must be pruned, got {:?}",
            s.forwarding_map
        );
    }

    #[test]
    fn drop_from_forwarding_handles_publisher_and_subscriber() {
        // Same pid acts as both publisher for one stream and subscriber of
        // another — the symmetry matters for generic cleanup correctness.
        let mut s = SfuSession::new(Uuid::new_v4());
        let gone = pid();
        let other = pid();

        s.forwarding_map
            .insert((gone, Mid::new()), vec![(other, Mid::new())]);
        s.forwarding_map
            .insert((other, Mid::new()), vec![(gone, Mid::new())]);

        s.drop_from_forwarding(gone);

        assert!(
            s.forwarding_map.is_empty(),
            "both roles of gone must be swept, got {:?}",
            s.forwarding_map
        );
    }

    #[test]
    fn drop_from_forwarding_unknown_pid_is_noop() {
        let mut s = SfuSession::new(Uuid::new_v4());
        let a = pid();
        let b = pid();
        s.forwarding_map.insert((a, Mid::new()), vec![(b, Mid::new())]);
        let snapshot = s.forwarding_map.clone();

        s.drop_from_forwarding(pid()); // unrelated

        assert_eq!(s.forwarding_map, snapshot);
    }

    #[test]
    fn track_out_open_mid_is_some_only_when_open() {
        let origin = pid();
        let origin_mid = Mid::new();
        let t = TrackOut {
            origin,
            origin_mid,
            kind: MediaKind::Audio,
            source: Source::Camera,
            state: TrackOutState::ToOpen,
        };
        assert_eq!(t.open_mid(), None);

        let nego_mid = Mid::new();
        let t = TrackOut {
            state: TrackOutState::Negotiating(nego_mid),
            ..t
        };
        assert_eq!(t.open_mid(), None);

        let open_mid = Mid::new();
        let t = TrackOut {
            state: TrackOutState::Open(open_mid),
            ..t
        };
        assert_eq!(t.open_mid(), Some(open_mid));
    }
}
