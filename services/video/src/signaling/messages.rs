use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    Error {
        message: String,
    },
}
