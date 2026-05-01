use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use str0m::Rtc;
use str0m::change::SdpPendingOffer;
use str0m::media::{KeyframeRequestKind, MediaKind, Mid};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::sfu::ParticipantId;
use crate::signaling::ServerMessage;

/// Minimum gap between consecutive PLI requests we send to the same
/// (publisher, mid). Coalesces bursts; well below any reasonable encoder's
/// own keyframe-suppression window so we never starve a real request.
const KEYFRAME_THROTTLE: Duration = Duration::from_millis(500);

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
    /// Timestamp of the last keyframe (PLI) we asked of (publisher, mid).
    /// Coalesces bursts when many subscribers open a track at once, e.g.
    /// 30 viewers acking a screen-share offer in the same RTT — without
    /// throttling each one would fire its own PLI and the publisher's
    /// encoder would emit multiple I-frames in a row, spiking bitrate.
    pub last_keyframe_at: HashMap<(ParticipantId, Mid), Instant>,
    /// Top-K audio publishers (by smoothed loudness EMA), refreshed by
    /// `recompute_top_audio` on every tick. Forwarding filter drops audio
    /// from publishers NOT in this set — saves a 30-person call from
    /// fan-out'ing 29 audio streams to every client when at most a handful
    /// are actually speaking at any moment. Empty set or fewer publishers
    /// than threshold means "no filter, forward everything".
    pub top_audio_publishers: HashSet<(ParticipantId, Mid)>,
}

/// Top-K audio publishers to forward when a session has many talkers.
/// 3 is the long-standing industry choice — Zoom, Meet, Teams all hover
/// around it. Subscribers see at most this many concurrent speakers, which
/// matches what humans can actually parse anyway.
pub const AUDIO_TOP_K: usize = 3;
/// Below this threshold (≤ K) we don't bother filtering — there's no win.
/// Above it, the filter trims the long tail of silent participants.
pub const AUDIO_FILTER_MIN_PUBLISHERS: usize = AUDIO_TOP_K + 1;

impl SfuSession {
    pub fn new(id: SessionId) -> Self {
        Self {
            id,
            participants: HashMap::new(),
            forwarding_map: HashMap::new(),
            last_keyframe_at: HashMap::new(),
            top_audio_publishers: HashSet::new(),
        }
    }

    /// Recompute which audio publishers should be forwarded this period.
    /// Sessions with ≤ AUDIO_TOP_K audio publishers fill `top_audio_publishers`
    /// with all of them (i.e. no actual filtering happens). Larger sessions
    /// keep only the loudest K — the rest go silent for subscribers until
    /// they speak up enough to climb the EMA.
    pub fn recompute_top_audio(&mut self) {
        let mut audio: Vec<(f32, ParticipantId, Mid)> = Vec::new();
        for (pid, p) in &self.participants {
            for t in &p.tracks_in {
                if t.kind == MediaKind::Audio {
                    audio.push((t.audio_loudness_ema, *pid, t.mid));
                }
            }
        }
        self.top_audio_publishers = pick_top_audio(audio);
    }

    /// Send a PLI keyframe request to (publisher, mid), coalescing bursts:
    /// no-op if we've already requested one within KEYFRAME_THROTTLE.
    /// Returns true if a request was actually sent. Used by both the
    /// "subscriber just opened" path and the "browser PLI from subscriber"
    /// path, so a 30-viewer fan-in doesn't translate to 30 I-frames.
    pub fn request_keyframe_throttled(
        &mut self,
        publisher_pid: ParticipantId,
        publisher_mid: Mid,
    ) -> bool {
        let now = Instant::now();
        if let Some(t) = self.last_keyframe_at.get(&(publisher_pid, publisher_mid))
            && now.duration_since(*t) < KEYFRAME_THROTTLE
        {
            return false;
        }
        let Some(publisher) = self.participants.get_mut(&publisher_pid) else {
            return false;
        };
        let Some(mut writer) = publisher.rtc.writer(publisher_mid) else {
            return false;
        };
        if writer.request_keyframe(None, KeyframeRequestKind::Pli).is_ok() {
            self.last_keyframe_at
                .insert((publisher_pid, publisher_mid), now);
            true
        } else {
            false
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
        // Throttle table is keyed by publisher; if the publisher leaves, clear
        // the stale entries so re-joins don't see a "recent" request that was
        // for a previous incarnation.
        self.last_keyframe_at
            .retain(|&(publisher, _), _| publisher != gone);
    }
}

/// Pure logic of recompute_top_audio: pick the top-K loudest audio
/// publishers, OR everyone if there are too few to bother filtering.
/// Pulled out as a free function so it's unit-testable without standing up
/// a full SfuParticipant (Rtc, ws_tx, etc).
pub fn pick_top_audio(
    mut audio: Vec<(f32, ParticipantId, Mid)>,
) -> HashSet<(ParticipantId, Mid)> {
    let mut out = HashSet::new();
    if audio.len() < AUDIO_FILTER_MIN_PUBLISHERS {
        for (_, pid, mid) in audio {
            out.insert((pid, mid));
        }
        return out;
    }
    audio.sort_by(|a, b| {
        b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
    });
    for (_, pid, mid) in audio.into_iter().take(AUDIO_TOP_K) {
        out.insert((pid, mid));
    }
    out
}

pub struct SfuParticipant {
    pub id: ParticipantId,
    pub user_id: String,
    pub rtc: Rtc,
    pub ws_tx: mpsc::Sender<ServerMessage>,
    /// Tracks this participant is publishing (incoming to SFU)
    pub tracks_in: Vec<TrackIn>,
    /// Tracks this participant is subscribing to (outgoing from SFU)
    pub tracks_out: Vec<TrackOut>,
    /// Pending SDP offer awaiting answer from client
    pub pending_offer: Option<SdpPendingOffer>,
    /// Client SDP offer that arrived while we still had `pending_offer` set
    /// (i.e. server's offer hadn't been answered yet). Drained inside
    /// `handle_answer` once we reach the stable state. Holds the latest
    /// offer; if the client sends two before answering ours, the older one
    /// is replaced (it's stale relative to the client's current PC state).
    pub queued_client_offer: Option<String>,
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
    /// Has the high simulcast layer (rid="h") ever been received from the
    /// publisher? Flips true on the first `h` packet and never goes back.
    /// Drives the start-up fallback: while `h` hasn't shown up yet (BWE
    /// hasn't ramped on the sender), the SFU forwards `l` so subscribers
    /// see something instead of a black screen for the first few seconds.
    /// Once `h` is live we drop `l` again — adaptive per-subscriber layer
    /// pick is a separate, larger feature.
    pub seen_high_layer: bool,
    /// Smoothed loudness on a 0..127 scale (higher = louder). Built from
    /// the audio-level RTP header extension (RFC 6464). Only updated for
    /// audio tracks; left at 0 for video. Drives the top-K speaker filter
    /// — see `SfuSession::top_audio_publishers`.
    pub audio_loudness_ema: f32,
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

    // ── Top-K audio selector ────────────────────────────────────

    fn audio_entry(loud: f32) -> (f32, ParticipantId, Mid) {
        (loud, Uuid::new_v4(), Mid::new())
    }

    #[test]
    fn pick_top_audio_passes_everyone_under_threshold() {
        // K = 3, threshold = K + 1 = 4. With 3 publishers, no filter.
        let entries = vec![audio_entry(10.0), audio_entry(80.0), audio_entry(40.0)];
        let pids_in: Vec<_> = entries.iter().map(|(_, p, m)| (*p, *m)).collect();
        let top = pick_top_audio(entries);
        assert_eq!(top.len(), 3);
        for k in pids_in {
            assert!(top.contains(&k));
        }
    }

    #[test]
    fn pick_top_audio_keeps_only_loudest_k_above_threshold() {
        // 5 publishers — top 3 loudest survive; bottom 2 get dropped.
        let q1 = audio_entry(5.0);
        let q2 = audio_entry(15.0);
        let m = audio_entry(50.0);
        let l1 = audio_entry(100.0);
        let l2 = audio_entry(95.0);
        let entries = vec![q1, q2, m, l1, l2];
        let top = pick_top_audio(entries);
        assert_eq!(top.len(), AUDIO_TOP_K);
        assert!(top.contains(&(l1.1, l1.2)));
        assert!(top.contains(&(l2.1, l2.2)));
        assert!(top.contains(&(m.1, m.2)));
        assert!(!top.contains(&(q1.1, q1.2)));
        assert!(!top.contains(&(q2.1, q2.2)));
    }

    #[test]
    fn pick_top_audio_handles_nan_without_panicking() {
        // NaN inputs would panic on a strict ordering. partial_cmp
        // fallback + Ordering::Equal keeps the sort stable.
        let entries = vec![
            audio_entry(f32::NAN),
            audio_entry(10.0),
            audio_entry(20.0),
            audio_entry(30.0),
            audio_entry(40.0),
        ];
        let top = pick_top_audio(entries);
        // Result: stable. We don't assert which 3 made it; only that the
        // function returned without aborting.
        assert_eq!(top.len(), AUDIO_TOP_K);
    }

    #[test]
    fn pick_top_audio_empty_input() {
        let top = pick_top_audio(Vec::new());
        assert!(top.is_empty());
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
