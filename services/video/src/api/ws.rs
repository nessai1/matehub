use axum::{
    Router,
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use uuid::Uuid;

use str0m::media::MediaKind;

use crate::sfu::{SfuCommand, Source};
use crate::signaling::{ClientMessage, ServerMessage};
use crate::state::{AppState, Participant, ParticipantState, SessionId};

pub fn routes() -> Router<AppState> {
    Router::new().route("/ws/{session_id}", get(ws_upgrade))
}

#[derive(Deserialize)]
struct WsQuery {
    #[allow(dead_code)]
    token: Option<String>,
    user_id: Option<String>,
}

async fn ws_upgrade(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    Query(query): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let user_id = query.user_id.unwrap_or_else(|| "anonymous".into());
    ws.on_upgrade(move |socket| handle_ws(socket, state, session_id, user_id))
}

async fn handle_ws(socket: WebSocket, state: AppState, session_id: SessionId, user_id: String) {
    let participant_id = Uuid::new_v4();
    let (ws_tx, mut ws_rx) = mpsc::unbounded_channel::<ServerMessage>();

    tracing::info!(%session_id, %participant_id, %user_id, "participant connecting");

    // Register participant in session state (for REST API visibility)
    {
        let mut inner = state.inner.lock();
        let Some(session) = inner.sessions.get_mut(&session_id) else {
            tracing::warn!(%session_id, "session not found for ws connection");
            return;
        };

        // Send existing participants to the new participant (+ their mute state)
        for p in session.participants.values() {
            let _ = ws_tx.send(ServerMessage::ParticipantJoined {
                participant_id: p.id,
                user_id: p.user_id.clone(),
            });
            // Send current mute state so new joiner knows who has camera/mic on
            if !p.video_muted {
                let _ = ws_tx.send(ServerMessage::ParticipantMuted {
                    participant_id: p.id,
                    kind: "video".into(),
                    muted: false,
                });
            }
            if !p.audio_muted {
                let _ = ws_tx.send(ServerMessage::ParticipantMuted {
                    participant_id: p.id,
                    kind: "audio".into(),
                    muted: false,
                });
            }
        }

        // Broadcast new participant to existing participants
        let join_msg = ServerMessage::ParticipantJoined {
            participant_id,
            user_id: user_id.clone(),
        };
        for p in session.participants.values() {
            let _ = p.ws_tx.send(join_msg.clone());
        }

        session.participants.insert(
            participant_id,
            Participant {
                id: participant_id,
                user_id: user_id.clone(),
                state: ParticipantState::Connecting,
                ws_tx: ws_tx.clone(),
                video_muted: true,
                audio_muted: true,
            },
        );
    }

    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Task: forward ServerMessages to WebSocket
    let send_task = tokio::spawn(async move {
        while let Some(msg) = ws_rx.recv().await {
            let text = match serde_json::to_string(&msg) {
                Ok(t) => t,
                Err(e) => {
                    tracing::error!("failed to serialize server message: {e}");
                    continue;
                }
            };
            if ws_sender.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    let sfu_tx = state.sfu_cmd_tx.clone();

    // Main loop: read client messages
    while let Some(Ok(msg)) = ws_receiver.next().await {
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };

        // Hot path: trickle ICE alone fires this dozens of times per join.
        // Keep it at debug so we don't allocate the truncated-string field
        // for every packet when RUST_LOG=info.
        tracing::debug!(%participant_id, bytes = text.len(), "WS raw message");

        let client_msg: ClientMessage = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                // Only on parse failure do we pay for the truncated preview.
                tracing::warn!(
                    %participant_id,
                    %e,
                    raw = %text.chars().take(200).collect::<String>(),
                    "failed to parse WS message"
                );
                let _ = ws_tx.send(ServerMessage::Error {
                    message: format!("invalid message: {e}"),
                });
                continue;
            }
        };

        match client_msg {
            ClientMessage::Join { sdp_offer } => {
                tracing::info!(%participant_id, "received SDP offer, sending to SFU");
                let _ = sfu_tx.send(SfuCommand::Join {
                    session_id,
                    participant_id,
                    user_id: user_id.clone(),
                    sdp_offer,
                    reply_tx: ws_tx.clone(),
                });

                // Update state
                let mut inner = state.inner.lock();
                if let Some(session) = inner.sessions.get_mut(&session_id)
                    && let Some(p) = session.participants.get_mut(&participant_id)
                {
                    p.state = ParticipantState::Connected;
                }
            }

            ClientMessage::Answer { sdp_answer } => {
                tracing::info!(%participant_id, "received SDP answer");
                let _ = sfu_tx.send(SfuCommand::Answer {
                    session_id,
                    participant_id,
                    sdp_answer,
                });
            }

            ClientMessage::IceCandidate {
                candidate,
                sdp_mid,
                sdp_mline_index: _,
            } => {
                // Trickle ICE is chatty — keep at debug to not flood logs.
                tracing::debug!(%participant_id, %candidate, "WS: forwarding ICE candidate to SFU");
                let _ = sfu_tx.send(SfuCommand::IceCandidate {
                    session_id,
                    participant_id,
                    candidate,
                    sdp_mid,
                });
            }

            ClientMessage::PublishTrack {
                source,
                kind,
                track_id: _,
            } => {
                let parsed_source = match source.as_str() {
                    "camera" => Some(Source::Camera),
                    "screen" => Some(Source::Screen),
                    _ => None,
                };
                let parsed_kind = match kind.as_str() {
                    "audio" => Some(MediaKind::Audio),
                    "video" => Some(MediaKind::Video),
                    _ => None,
                };
                match (parsed_source, parsed_kind) {
                    (Some(source), Some(kind)) => {
                        tracing::info!(%participant_id, ?source, ?kind, "publish_track hint");
                        let _ = sfu_tx.send(SfuCommand::PublishTrack {
                            session_id,
                            participant_id,
                            source,
                            kind,
                        });
                    }
                    _ => {
                        tracing::warn!(%participant_id, %source, %kind, "unknown publish_track kind/source");
                    }
                }
            }

            ClientMessage::Offer { sdp_offer } => {
                tracing::info!(%participant_id, "received client-initiated SDP offer");
                let _ = sfu_tx.send(SfuCommand::ClientOffer {
                    session_id,
                    participant_id,
                    sdp_offer,
                });
            }

            ClientMessage::MuteChanged { kind, muted } => {
                tracing::info!(%participant_id, %kind, %muted, "mute changed");
                let mut inner = state.inner.lock();
                if let Some(session) = inner.sessions.get_mut(&session_id) {
                    // Persist mute state so new joiners get it
                    if let Some(me) = session.participants.get_mut(&participant_id) {
                        match kind.as_str() {
                            "video" => me.video_muted = muted,
                            "audio" => me.audio_muted = muted,
                            _ => {}
                        }
                    }
                    // Broadcast to other participants
                    let msg = ServerMessage::ParticipantMuted {
                        participant_id,
                        kind,
                        muted,
                    };
                    for (pid, p) in &session.participants {
                        if *pid != participant_id {
                            let _ = p.ws_tx.send(msg.clone());
                        }
                    }
                }
            }

            ClientMessage::Leave => {
                tracing::info!(%participant_id, %user_id, "participant leaving");
                let _ = sfu_tx.send(SfuCommand::Leave {
                    session_id,
                    participant_id,
                });
                break;
            }
        }
    }

    // Cleanup
    send_task.abort();

    // Send leave to SFU (in case WS dropped without explicit leave)
    let _ = sfu_tx.send(SfuCommand::Leave {
        session_id,
        participant_id,
    });

    // Remove from REST API state (single lock scope to avoid TOCTOU race)
    {
        let mut inner = state.inner.lock();
        if let Some(session) = inner.sessions.get_mut(&session_id) {
            session.participants.remove(&participant_id);

            let leave_msg = ServerMessage::ParticipantLeft {
                participant_id,
                user_id: user_id.clone(),
            };
            for p in session.participants.values() {
                let _ = p.ws_tx.send(leave_msg.clone());
            }

            if session.participants.is_empty() {
                let session = inner.sessions.remove(&session_id).unwrap();
                inner.channel_to_session.remove(&session.channel_id);
                tracing::info!(%session_id, "session destroyed (last participant left)");
            }
        }
    }

    tracing::info!(%session_id, %participant_id, %user_id, "participant disconnected");
}
