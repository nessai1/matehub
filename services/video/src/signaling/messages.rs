use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Maps a stream_id in SDP to its origin participant, with source and kind
/// so the SDK can render the right tile (camera vs screen) without DOM-level
/// heuristics. `source` values: "camera" | "screen". `kind`: "audio" | "video".
#[derive(Debug, Serialize, Clone)]
pub struct TrackMapping {
    pub stream_id: String,
    pub participant_id: Uuid,
    pub user_id: String,
    pub source: &'static str,
    pub kind: &'static str,
}

/// Client -> Server messages
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Join {
        sdp_offer: String,
    },
    Answer {
        sdp_answer: String,
    },
    IceCandidate {
        candidate: String,
        sdp_mid: Option<String>,
        #[allow(dead_code)] // deserialized from JSON, forwarded to SFU
        sdp_mline_index: Option<u16>,
    },
    MuteChanged {
        kind: String,
        muted: bool,
    },
    /// Deafen state changed (Discord-style "I can't hear anyone"). The SFU
    /// doesn't actually do anything different on its forwarding side —
    /// audio still flows to the deafened participant's PeerConnection, the
    /// client just has its GainNode at 0. But broadcasting the flag lets
    /// every other client render a deafened-headphone icon on this
    /// participant's tile, which is the cue the rest of the call needs
    /// ("don't bother asking, they can't hear you right now").
    DeafenChanged {
        deafened: bool,
    },
    /// Source-hint for the next media track(s) in the upcoming Offer.
    /// Must arrive BEFORE the Offer (WS preserves order within one socket).
    /// `source`: "camera" | "screen", `kind`: "audio" | "video".
    /// `track_id` is informational (MediaStreamTrack.id) — the server uses
    /// FIFO matching against the next MediaAdded event of the same kind.
    PublishTrack {
        source: String,
        kind: String,
        #[allow(dead_code)] // reserved for future explicit track→mid mapping
        track_id: Option<String>,
    },
    /// Client-initiated SDP renegotiation (e.g. publisher added a screen
    /// track via `addTrack`). Server answers with a standard Answer.
    Offer {
        sdp_offer: String,
    },
    Leave,
}

/// Server -> Client messages
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Answer {
        sdp_answer: String,
        participant_id: Uuid,
    },
    Offer {
        sdp_offer: String,
        /// Maps stream_id (used in SDP) to participant info for track matching
        #[serde(skip_serializing_if = "Option::is_none")]
        tracks: Option<Vec<TrackMapping>>,
    },
    #[allow(dead_code)] // will be used for trickle ICE from SFU to client
    IceCandidate {
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    },
    ParticipantJoined {
        participant_id: Uuid,
        user_id: String,
    },
    ParticipantLeft {
        participant_id: Uuid,
        user_id: String,
    },
    ParticipantMuted {
        participant_id: Uuid,
        kind: String,
        muted: bool,
    },
    /// Broadcast counterpart to `ClientMessage::DeafenChanged`. State
    /// also lives on the SFU side (see `SfuParticipant::deafened`) so a
    /// late-joining client gets each existing participant's current
    /// deafen flag in the bootstrap-replay burst, same shape as
    /// `ParticipantMuted`.
    ParticipantDeafened {
        participant_id: Uuid,
        deafened: bool,
    },
    Error {
        message: String,
    },
    /// SFU is dropping this client because the same user joined this
    /// session from another tab/device. Client should leave the call UI
    /// and surface the reason — without this signal, the kicked tab
    /// would stay on a dead Rtc until the WS times out.
    ForceDisconnected {
        reason: String,
    },
}
