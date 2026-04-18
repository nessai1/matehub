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
    Error {
        message: String,
    },
}
