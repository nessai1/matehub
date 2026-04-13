/// Unit tests for signaling message serialization/deserialization.
use matehub_video::signaling::{ClientMessage, ServerMessage};
use uuid::Uuid;

#[test]
fn client_join_deserializes() {
    let json = r#"{"type":"join","sdp_offer":"v=0\r\n"}"#;
    let msg: ClientMessage = serde_json::from_str(json).unwrap();
    assert!(matches!(msg, ClientMessage::Join { sdp_offer } if sdp_offer == "v=0\r\n"));
}

#[test]
fn client_answer_deserializes() {
    let json = r#"{"type":"answer","sdp_answer":"v=0\r\n"}"#;
    let msg: ClientMessage = serde_json::from_str(json).unwrap();
    assert!(matches!(msg, ClientMessage::Answer { .. }));
}

#[test]
fn client_ice_candidate_deserializes() {
    let json = r#"{"type":"ice_candidate","candidate":"candidate:123 1 udp 2130706431 192.168.1.1 5000 typ host","sdp_mid":"0","sdp_mline_index":0}"#;
    let msg: ClientMessage = serde_json::from_str(json).unwrap();
    assert!(matches!(msg, ClientMessage::IceCandidate { .. }));
}

#[test]
fn client_ice_candidate_optional_fields() {
    let json = r#"{"type":"ice_candidate","candidate":"candidate:123 1 udp 2130706431 192.168.1.1 5000 typ host"}"#;
    let msg: ClientMessage = serde_json::from_str(json).unwrap();
    match msg {
        ClientMessage::IceCandidate {
            sdp_mid,
            sdp_mline_index,
            ..
        } => {
            assert!(sdp_mid.is_none());
            assert!(sdp_mline_index.is_none());
        }
        _ => panic!("expected IceCandidate"),
    }
}

#[test]
fn client_leave_deserializes() {
    let json = r#"{"type":"leave"}"#;
    let msg: ClientMessage = serde_json::from_str(json).unwrap();
    assert!(matches!(msg, ClientMessage::Leave));
}

#[test]
fn client_unknown_type_fails() {
    let json = r#"{"type":"explode"}"#;
    assert!(serde_json::from_str::<ClientMessage>(json).is_err());
}

#[test]
fn client_missing_type_fails() {
    let json = r#"{"sdp_offer":"v=0"}"#;
    assert!(serde_json::from_str::<ClientMessage>(json).is_err());
}

#[test]
fn server_answer_serializes_with_type_tag() {
    let msg = ServerMessage::Answer {
        sdp_answer: "v=0\r\n".into(),
        participant_id: Uuid::nil(),
    };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["type"], "answer");
    assert_eq!(json["sdp_answer"], "v=0\r\n");
    assert!(json["participant_id"].is_string());
}

#[test]
fn server_offer_serializes() {
    let msg = ServerMessage::Offer {
        sdp_offer: "v=0\r\n".into(),
        tracks: None,
    };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["type"], "offer");
}

#[test]
fn server_participant_joined_serializes() {
    let msg = ServerMessage::ParticipantJoined {
        participant_id: Uuid::nil(),
        user_id: "alice".into(),
    };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["type"], "participant_joined");
    assert_eq!(json["user_id"], "alice");
}

#[test]
fn server_participant_left_serializes() {
    let msg = ServerMessage::ParticipantLeft {
        participant_id: Uuid::nil(),
        user_id: "bob".into(),
    };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["type"], "participant_left");
    assert_eq!(json["user_id"], "bob");
}

#[test]
fn server_error_serializes() {
    let msg = ServerMessage::Error {
        message: "something broke".into(),
    };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["type"], "error");
    assert_eq!(json["message"], "something broke");
}

// ── Mute signaling (Stage 2) ────────────────────

#[test]
fn client_mute_changed_deserializes() {
    let json = r#"{"type":"mute_changed","kind":"video","muted":true}"#;
    let msg: ClientMessage = serde_json::from_str(json).unwrap();
    match msg {
        ClientMessage::MuteChanged { kind, muted } => {
            assert_eq!(kind, "video");
            assert!(muted);
        }
        _ => panic!("expected MuteChanged"),
    }
}

#[test]
fn client_mute_changed_audio_unmute() {
    let json = r#"{"type":"mute_changed","kind":"audio","muted":false}"#;
    let msg: ClientMessage = serde_json::from_str(json).unwrap();
    match msg {
        ClientMessage::MuteChanged { kind, muted } => {
            assert_eq!(kind, "audio");
            assert!(!muted);
        }
        _ => panic!("expected MuteChanged"),
    }
}

#[test]
fn server_participant_muted_serializes() {
    let msg = ServerMessage::ParticipantMuted {
        participant_id: Uuid::nil(),
        kind: "video".into(),
        muted: false,
    };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["type"], "participant_muted");
    assert_eq!(json["kind"], "video");
    assert_eq!(json["muted"], false);
    assert!(json["participant_id"].is_string());
}

#[test]
fn server_participant_muted_audio() {
    let msg = ServerMessage::ParticipantMuted {
        participant_id: Uuid::nil(),
        kind: "audio".into(),
        muted: true,
    };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["type"], "participant_muted");
    assert_eq!(json["kind"], "audio");
    assert_eq!(json["muted"], true);
}
