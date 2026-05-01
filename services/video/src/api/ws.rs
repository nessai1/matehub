use axum::{
    Router,
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{Instrument, Span, field::Empty};
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
    /// Required. Either a real JWT (verified via matehub_common::auth) or a
    /// dev-mode shortcut "dev-{username}-token" — same pattern as the hub
    /// service. user_id is derived from the verified claims, NOT trusted
    /// from a separate query param.
    token: Option<String>,
}

/// Validate a WS token and return the authenticated user_id. None means
/// reject the upgrade. Sources of truth, in order:
///   * dev shortcut: "dev-{username}-token" → user_id = "{username}".
///     Matches services/hub/src/auth.rs; lets local tests skip JWT issuance.
///   * Real JWT: verify_token → claims.sub (Snowflake i64) as decimal string.
fn authenticate(token: Option<&str>) -> Option<String> {
    let token = token?;
    if let Some(rest) = token.strip_prefix("dev-")
        && let Some(username) = rest.strip_suffix("-token")
        && !username.is_empty()
    {
        return Some(username.to_string());
    }
    match matehub_common::auth::verify_token(token) {
        Ok(claims) => Some(claims.sub.to_string()),
        Err(e) => {
            tracing::warn!(error = %e, "WS token verification failed");
            None
        }
    }
}

const VOICE_OCCUPANCY_SUBJECT: &str = "voice.occupancy";
const VOICE_MUTE_SUBJECT: &str = "voice.mute";

/// Publish an audio/video mute toggle for a user. Lets the hub fan it out
/// to all hub-wide presence WS clients — without this, peers in OTHER
/// voice channels don't see Alice's mic state because the SFU's
/// in-session `participant_muted` broadcast never reaches them.
async fn publish_mute(
    nats: &async_nats::Client,
    hub_id: i64,
    user_id: i64,
    kind: &str,
    muted: bool,
) {
    let payload = serde_json::json!({
        "hub_id": hub_id.to_string(),
        "user_id": user_id.to_string(),
        "kind": kind,
        "muted": muted,
    });
    let bytes = match serde_json::to_vec(&payload) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "failed to encode voice.mute event");
            return;
        }
    };
    if let Err(e) = nats.publish(VOICE_MUTE_SUBJECT.to_string(), bytes.into()).await {
        tracing::warn!(error = %e, "failed to publish voice.mute event");
    }
}

/// Publish a `voice.occupancy` event. `channel_id = None` signals "left".
/// `call_id` is the SFU session id — carried so the hub can stitch this
/// event into the correlated call-trace span without extra lookups.
/// Failures are logged and swallowed — the call must not block on NATS.
///
/// ID fields are wired as strings (Snowflakes exceed JS safe-int). Hub's
/// `OccupancyEvent` deserialises them via `serde_i64::as_string`.
async fn publish_occupancy(
    nats: &async_nats::Client,
    hub_id: i64,
    user_id: i64,
    channel_id: Option<i64>,
    call_id: Uuid,
) {
    let payload = serde_json::json!({
        "hub_id": hub_id.to_string(),
        "user_id": user_id.to_string(),
        "channel_id": channel_id.map(|c| c.to_string()),
        "call_id": call_id,
    });
    let bytes = match serde_json::to_vec(&payload) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "failed to encode voice.occupancy event");
            return;
        }
    };
    if let Err(e) = nats
        .publish(VOICE_OCCUPANCY_SUBJECT.to_string(), bytes.into())
        .await
    {
        tracing::warn!(error = %e, "failed to publish voice.occupancy event");
    }
}

async fn ws_upgrade(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    Query(query): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(user_id) = authenticate(query.token.as_deref()) else {
        metrics::counter!("matehub_video_ws_auth_rejects_total").increment(1);
        return (StatusCode::UNAUTHORIZED, "missing or invalid token").into_response();
    };
    let user_snowflake = user_id.parse::<i64>().ok();

    let span = tracing::info_span!(
        "call",
        call_id = %session_id,
        user_id = %user_id,
        hub_id = Empty,
        participant_id = Empty,
    );

    ws.on_upgrade(move |socket| {
        handle_ws(socket, state, session_id, user_id, user_snowflake).instrument(span)
    })
    .into_response()
}

/// Outbound WS buffer size per participant. Each ServerMessage is at most
/// a renegotiation Offer (~50KB SDP) but most are tiny. 256 messages ≈ a
/// small SDP burst with headroom; well under "this connection is broken,
/// give up".
const WS_TX_BUFFER: usize = 256;

/// Drop-on-full helper for the per-WS broadcast channel. Slow consumers
/// don't get to back-pressure the SFU.
fn ws_send_or_drop(tx: &mpsc::Sender<ServerMessage>, msg: ServerMessage) {
    if let Err(e) = tx.try_send(msg) {
        // SendError on closed-channel is the legit "WS already disconnected"
        // case — silent. Full is the alertable signal.
        if matches!(e, mpsc::error::TrySendError::Full(_)) {
            metrics::counter!("matehub_video_ws_send_drops_total").increment(1);
        }
    }
}

/// Drop-on-full helper for the SFU pool. Routes by session id internally
/// (consistent hash), so every command lands on the shard owning that
/// session. Counter increment happens inside `SfuPool::send`.
fn sfu_send_or_drop(pool: &crate::sfu::SfuPool, sid: SessionId, cmd: SfuCommand) {
    pool.send(sid, cmd);
}

/// Derive a stable participant id from (session_id, user_id). Reconnect
/// from the same user lands on the same slot — `handle_join` detects the
/// existing participant and does a graceful replace (drops the old Rtc,
/// rebuilds tracks). Without this, a transient WS drop left a zombie that
/// kept publishing audio for the full ~12s zombie window, doubling the
/// caller's voice in everyone else's mix.
///
/// "anonymous" stays on v4 (collision-free) — only authenticated sessions
/// get the deterministic id. Once JWT auth is mandatory we can drop this
/// branch.
fn derive_participant_id(session_id: SessionId, user_id: &str) -> Uuid {
    if user_id == "anonymous" {
        Uuid::new_v4()
    } else {
        Uuid::new_v5(&session_id, user_id.as_bytes())
    }
}

async fn handle_ws(
    socket: WebSocket,
    state: AppState,
    session_id: SessionId,
    user_id: String,
    user_snowflake: Option<i64>,
) {
    let participant_id = derive_participant_id(session_id, &user_id);
    Span::current().record("participant_id", tracing::field::display(&participant_id));
    let (ws_tx, mut ws_rx) = mpsc::channel::<ServerMessage>(WS_TX_BUFFER);

    tracing::info!("participant connecting");

    // Snapshot these before we take the lock so the NATS publish below doesn't
    // need the AppStateInner guard.
    let (channel_id, hub_id) = {
        let inner = state.inner.lock();
        let Some(session) = inner.sessions.get(&session_id) else {
            tracing::warn!("session not found for ws connection");
            return;
        };
        (session.channel_id, session.hub_id)
    };
    Span::current().record("hub_id", tracing::field::display(&hub_id));

    // Register participant in session state (for REST API visibility)
    {
        let mut inner = state.inner.lock();
        let Some(session) = inner.sessions.get_mut(&session_id) else {
            tracing::warn!("session not found for ws connection");
            return;
        };

        // Send existing participants to the new participant (+ their mute state)
        for p in session.participants.values() {
            ws_send_or_drop(
                &ws_tx,
                ServerMessage::ParticipantJoined {
                    participant_id: p.id,
                    user_id: p.user_id.clone(),
                },
            );
            // Send current mute state so new joiner knows who has camera/mic on
            if !p.video_muted {
                ws_send_or_drop(
                    &ws_tx,
                    ServerMessage::ParticipantMuted {
                        participant_id: p.id,
                        kind: "video".into(),
                        muted: false,
                    },
                );
            }
            if !p.audio_muted {
                ws_send_or_drop(
                    &ws_tx,
                    ServerMessage::ParticipantMuted {
                        participant_id: p.id,
                        kind: "audio".into(),
                        muted: false,
                    },
                );
            }
        }

        // Broadcast new participant to existing participants
        let join_msg = ServerMessage::ParticipantJoined {
            participant_id,
            user_id: user_id.clone(),
        };
        for p in session.participants.values() {
            ws_send_or_drop(&p.ws_tx, join_msg.clone());
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
        metrics::counter!("matehub_video_participant_joins_total").increment(1);
    }

    // Tell the hub: this user is now in this voice channel.
    if let (Some(nats), Some(uid)) = (state.nats.as_ref(), user_snowflake) {
        publish_occupancy(nats, hub_id, uid, Some(channel_id), session_id).await;
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

    let sfu_pool = state.sfu_pool.clone();

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
                ws_send_or_drop(
                    &ws_tx,
                    ServerMessage::Error {
                        message: format!("invalid message: {e}"),
                    },
                );
                continue;
            }
        };

        match client_msg {
            ClientMessage::Join { sdp_offer } => {
                tracing::info!(%participant_id, "received SDP offer, sending to SFU");
                sfu_send_or_drop(
                    &sfu_pool,
                    session_id,
                    SfuCommand::Join {
                        session_id,
                        participant_id,
                        user_id: user_id.clone(),
                        sdp_offer,
                        reply_tx: ws_tx.clone(),
                    },
                );

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
                sfu_send_or_drop(
                    &sfu_pool,
                    session_id,
                    SfuCommand::Answer {
                        session_id,
                        participant_id,
                        sdp_answer,
                    },
                );
            }

            ClientMessage::IceCandidate {
                candidate,
                sdp_mid,
                sdp_mline_index: _,
            } => {
                // Trickle ICE is chatty — keep at debug to not flood logs.
                tracing::debug!(%participant_id, %candidate, "WS: forwarding ICE candidate to SFU");
                sfu_send_or_drop(
                    &sfu_pool,
                    session_id,
                    SfuCommand::IceCandidate {
                        session_id,
                        participant_id,
                        candidate,
                        sdp_mid,
                    },
                );
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
                        sfu_send_or_drop(
                    &sfu_pool,
                    session_id,
                            SfuCommand::PublishTrack {
                                session_id,
                                participant_id,
                                source,
                                kind,
                            },
                        );
                    }
                    _ => {
                        tracing::warn!(%participant_id, %source, %kind, "unknown publish_track kind/source");
                    }
                }
            }

            ClientMessage::Offer { sdp_offer } => {
                tracing::info!(%participant_id, "received client-initiated SDP offer");
                sfu_send_or_drop(
                    &sfu_pool,
                    session_id,
                    SfuCommand::ClientOffer {
                        session_id,
                        participant_id,
                        sdp_offer,
                    },
                );
            }

            ClientMessage::MuteChanged { kind, muted } => {
                tracing::info!(%participant_id, %kind, %muted, "mute changed");
                {
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
                        // In-session broadcast — peers in this call get
                        // immediate per-track update via SFU's WS.
                        let msg = ServerMessage::ParticipantMuted {
                            participant_id,
                            kind: kind.clone(),
                            muted,
                        };
                        for (pid, p) in &session.participants {
                            if *pid != participant_id {
                                ws_send_or_drop(&p.ws_tx, msg.clone());
                            }
                        }
                    }
                }
                // Hub-wide fanout — peers in OTHER voice channels (or just
                // viewing the sidebar) get the update via NATS → hub →
                // presence-WS. Without this the mic indicator next to a
                // participant's name only updates while you're in their call.
                if let (Some(nats), Some(uid)) =
                    (state.nats.as_ref(), user_snowflake)
                {
                    publish_mute(nats, hub_id, uid, &kind, muted).await;
                }
            }

            ClientMessage::Leave => {
                tracing::info!(%participant_id, %user_id, "participant leaving");
                sfu_send_or_drop(
                    &sfu_pool,
                    session_id,
                    SfuCommand::Leave {
                        session_id,
                        participant_id,
                    },
                );
                break;
            }
        }
    }

    // Cleanup
    send_task.abort();

    // Send leave to SFU (in case WS dropped without explicit leave)
    sfu_send_or_drop(
                    &sfu_pool,
                    session_id,
        SfuCommand::Leave {
            session_id,
            participant_id,
        },
    );

    // Remove from REST API state (single lock scope to avoid TOCTOU race).
    // Reconnect twist: with derived participant_id, a fresh WS from the same
    // user overwrites our slot. If our ws_tx no longer matches the slot's
    // tx, this handler is the OLD connection — don't yank the new one out
    // from under itself. `same_channel` compares the underlying mpsc handle.
    {
        let mut inner = state.inner.lock();
        if let Some(session) = inner.sessions.get_mut(&session_id) {
            let still_ours = session
                .participants
                .get(&participant_id)
                .map(|p| p.ws_tx.same_channel(&ws_tx))
                .unwrap_or(false);
            if still_ours {
                session.participants.remove(&participant_id);

                let leave_msg = ServerMessage::ParticipantLeft {
                    participant_id,
                    user_id: user_id.clone(),
                };
                for p in session.participants.values() {
                    ws_send_or_drop(&p.ws_tx, leave_msg.clone());
                }

                if session.participants.is_empty() {
                    let session = inner.sessions.remove(&session_id).unwrap();
                    inner.channel_to_session.remove(&session.channel_id);
                    metrics::counter!("matehub_video_session_destroys_total").increment(1);
                    tracing::info!("session destroyed (last participant left)");
                }
                metrics::counter!("matehub_video_participant_leaves_total").increment(1);
            } else {
                tracing::debug!(
                    %session_id,
                    %participant_id,
                    "stale WS handler exiting; slot already owned by reconnected session"
                );
            }
        }
    }

    // Tell the hub: this user left the voice channel.
    if let (Some(nats), Some(uid)) = (state.nats.as_ref(), user_snowflake) {
        publish_occupancy(nats, hub_id, uid, None, session_id).await;
    }

    tracing::info!("participant disconnected");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_participant_id_is_stable_for_same_user() {
        let session = Uuid::new_v4();
        let a = derive_participant_id(session, "alice");
        let b = derive_participant_id(session, "alice");
        assert_eq!(a, b, "same (session, user) must produce same pid");
    }

    #[test]
    fn derive_participant_id_differs_per_user() {
        let session = Uuid::new_v4();
        assert_ne!(
            derive_participant_id(session, "alice"),
            derive_participant_id(session, "bob"),
        );
    }

    #[test]
    fn derive_participant_id_differs_per_session() {
        // Same user in different sessions must NOT collide — otherwise
        // session A's ufrag map could be hit by session B's STUN packets.
        let s1 = Uuid::new_v4();
        let s2 = Uuid::new_v4();
        assert_ne!(
            derive_participant_id(s1, "alice"),
            derive_participant_id(s2, "alice"),
        );
    }

    #[test]
    fn derive_participant_id_anonymous_is_random() {
        // Two anonymous connects must NOT collapse onto each other —
        // otherwise the second WS would clobber the first's slot
        // unintentionally.
        let session = Uuid::new_v4();
        let a = derive_participant_id(session, "anonymous");
        let b = derive_participant_id(session, "anonymous");
        assert_ne!(a, b);
    }

    #[test]
    fn authenticate_dev_shortcut_extracts_username() {
        assert_eq!(
            authenticate(Some("dev-alice-token")).as_deref(),
            Some("alice"),
        );
    }

    #[test]
    fn authenticate_rejects_missing_token() {
        assert!(authenticate(None).is_none());
    }

    #[test]
    fn authenticate_rejects_dev_with_empty_username() {
        // "dev--token" parses to empty username; must not authenticate.
        assert!(authenticate(Some("dev--token")).is_none());
    }

    #[test]
    fn authenticate_rejects_garbage() {
        // Random non-JWT, non-dev string. verify_token returns Err.
        assert!(authenticate(Some("not-a-token")).is_none());
    }
}
