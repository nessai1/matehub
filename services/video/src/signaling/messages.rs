use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Maps a stream_id in SDP to its origin participant
#[derive(Debug, Serialize, Clone)]
pub struct TrackMapping {
    pub stream_id: String,
    pub participant_id: Uuid,
    pub user_id: String,
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
        sdp_mline_index: Option<u16>,
    },
    MuteChanged {
        kind: String,
        muted: bool,
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
