//! Зеркало WS-протокола video-сервиса с точки зрения клиента.
//!
//! Источник истины — `services/video/src/signaling/messages.rs`: там
//! `ClientMessage` — Deserialize, здесь — Serialize (и наоборот для
//! `ServerMessage`). Юнит-тесты ниже фиксируют wire-формат, чтобы дрейф
//! одной из сторон ловился на `cargo test`, а не на живом звонке.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Клиент → сервер. Тег — поле `type` в snake_case.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Join {
        sdp_offer: String,
    },
    Answer {
        sdp_answer: String,
    },
    #[allow(dead_code)] // кандидаты кладём в оффер целиком; оставлено для trickle
    IceCandidate {
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    },
    MuteChanged {
        kind: String,
        muted: bool,
    },
    /// Source-хинт для следующего MediaAdded того же kind. Строго ДО оффера,
    /// и только ПОСЛЕ join (до join сервер дропает хинт: участника нет).
    PublishTrack {
        source: String,
        kind: String,
        track_id: Option<String>,
    },
    /// Клиентская ренеготиация (добавили screen-трек).
    Offer {
        sdp_offer: String,
    },
    Leave,
}

/// Маппинг stream_id → участник/источник в серверном оффере.
#[derive(Debug, Clone, Deserialize)]
pub struct TrackMapping {
    pub stream_id: String,
    pub participant_id: Uuid,
    pub user_id: String,
    pub source: String,
    pub kind: String,
}

/// Сервер → клиент.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Answer {
        sdp_answer: String,
        participant_id: Uuid,
    },
    Offer {
        sdp_offer: String,
        #[serde(default)]
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
    ParticipantDeafened {
        participant_id: Uuid,
        deafened: bool,
    },
    Error {
        message: String,
    },
    ForceDisconnected {
        reason: String,
    },
}

/// WS URL сигналинга: `ws(s)://…/ws/{session_id}?token=…[&device=…]`.
///
/// `base_url` — HTTP-база video-сервиса (`http://host:4000` или
/// `https://host/api/video`). JWT и dev-токены состоят из URL-безопасных
/// символов (`[A-Za-z0-9._-]`), поэтому токен вставляется без кодирования;
/// на прочие символы отвечаем ошибкой, а не молча ломаем query.
///
/// `device` — квалификатор вспомогательного соединения того же юзера
/// (сервер знает `screen`): participant_id деривируется из
/// (session, user, device), и companion не выбивает основную сессию.
pub fn ws_url(
    base_url: &str,
    session_id: &str,
    token: &str,
    device: Option<&str>,
) -> Result<String, String> {
    let ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    if token.is_empty() || !token.chars().all(ok) {
        return Err("token contains characters unsafe for a query string".into());
    }
    if let Some(d) = device
        && (d.is_empty() || !d.chars().all(|c: char| c.is_ascii_lowercase()))
    {
        return Err(format!("invalid device qualifier: {d:?}"));
    }
    let base = base_url.trim_end_matches('/');
    let ws_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        return Err(format!(
            "base url must start with http(s)://, got: {base_url}"
        ));
    };
    let device_q = device.map(|d| format!("&device={d}")).unwrap_or_default();
    Ok(format!("{ws_base}/ws/{session_id}?token={token}{device_q}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::{Value, json};

    fn to_value(msg: &ClientMessage) -> Value {
        serde_json::to_value(msg).unwrap()
    }

    #[test]
    fn client_messages_match_server_wire_format() {
        assert_eq!(
            to_value(&ClientMessage::Join {
                sdp_offer: "v=0".into()
            }),
            json!({"type": "join", "sdp_offer": "v=0"})
        );
        assert_eq!(
            to_value(&ClientMessage::Answer {
                sdp_answer: "v=0".into()
            }),
            json!({"type": "answer", "sdp_answer": "v=0"})
        );
        assert_eq!(
            to_value(&ClientMessage::PublishTrack {
                source: "screen".into(),
                kind: "video".into(),
                track_id: None,
            }),
            json!({"type": "publish_track", "source": "screen", "kind": "video", "track_id": null})
        );
        assert_eq!(
            to_value(&ClientMessage::Offer {
                sdp_offer: "v=0".into()
            }),
            json!({"type": "offer", "sdp_offer": "v=0"})
        );
        assert_eq!(
            to_value(&ClientMessage::MuteChanged {
                kind: "video".into(),
                muted: false,
            }),
            json!({"type": "mute_changed", "kind": "video", "muted": false})
        );
        assert_eq!(to_value(&ClientMessage::Leave), json!({"type": "leave"}));
        assert_eq!(
            to_value(&ClientMessage::IceCandidate {
                candidate: "candidate:1 1 udp 2130706431 10.0.0.1 4001 typ host".into(),
                sdp_mid: Some("0".into()),
                sdp_mline_index: Some(0),
            }),
            json!({
                "type": "ice_candidate",
                "candidate": "candidate:1 1 udp 2130706431 10.0.0.1 4001 typ host",
                "sdp_mid": "0",
                "sdp_mline_index": 0
            })
        );
    }

    #[test]
    fn server_messages_parse_from_wire() {
        let pid = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
        let msg: ServerMessage = serde_json::from_str(&format!(
            r#"{{"type":"answer","sdp_answer":"v=0","participant_id":"{pid}"}}"#
        ))
        .unwrap();
        assert!(matches!(msg, ServerMessage::Answer { .. }));

        let msg: ServerMessage = serde_json::from_str(&format!(
            r#"{{"type":"offer","sdp_offer":"v=0","tracks":[{{"stream_id":"x-screen-video","participant_id":"{pid}","user_id":"alice","source":"screen","kind":"video"}}]}}"#
        ))
        .unwrap();
        match msg {
            ServerMessage::Offer { tracks, .. } => {
                let tracks = tracks.unwrap();
                assert_eq!(tracks[0].source, "screen");
            }
            other => panic!("expected offer, got {other:?}"),
        }

        // Оффер без tracks — поле опциональное.
        let msg: ServerMessage =
            serde_json::from_str(r#"{"type":"offer","sdp_offer":"v=0"}"#).unwrap();
        assert!(matches!(msg, ServerMessage::Offer { tracks: None, .. }));

        let msg: ServerMessage =
            serde_json::from_str(r#"{"type":"force_disconnected","reason":"joined_elsewhere"}"#)
                .unwrap();
        assert!(matches!(msg, ServerMessage::ForceDisconnected { .. }));

        let msg: ServerMessage =
            serde_json::from_str(r#"{"type":"error","message":"boom"}"#).unwrap();
        assert!(matches!(msg, ServerMessage::Error { .. }));
    }

    #[test]
    fn ws_url_builds_and_validates() {
        assert_eq!(
            ws_url("http://localhost:4000", "abc", "dev-alice-token", None).unwrap(),
            "ws://localhost:4000/ws/abc?token=dev-alice-token"
        );
        assert_eq!(
            ws_url(
                "https://hub.example.com/api/video/",
                "abc",
                "a.b-c_d",
                Some("screen")
            )
            .unwrap(),
            "wss://hub.example.com/api/video/ws/abc?token=a.b-c_d&device=screen"
        );
        assert!(ws_url("ftp://x", "abc", "t", None).is_err());
        assert!(ws_url("http://x", "abc", "has space", None).is_err());
        assert!(ws_url("http://x", "abc", "t", Some("SCREEN")).is_err());
        assert!(ws_url("http://x", "abc", "", None).is_err());
    }
}
