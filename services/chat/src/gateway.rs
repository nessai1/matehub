use std::future;
use std::time::Duration;

use axum::{Router, extract::State, response::IntoResponse, routing::get};
use fastwebsockets::upgrade::IncomingUpgrade;
use fastwebsockets::{
    FragmentCollectorRead, Frame, OpCode, Payload, WebSocketError, WebSocketWrite,
};
use futures_util::StreamExt;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

use crate::api::AppState;
use crate::auth;
use crate::models::{GatewayCommand, GatewayEvent, Opcode, events};
use crate::session::SessionHandle;

const HEARTBEAT_INTERVAL_MS: u64 = 41_000;
const HEARTBEAT_TIMEOUT_MS: u64 = 45_000;

pub fn routes(state: AppState) -> Router {
    Router::new()
        .route("/gateway", get(ws_upgrade))
        .with_state(state)
}

async fn ws_upgrade(State(state): State<AppState>, ws: IncomingUpgrade) -> impl IntoResponse {
    let (response, fut) = ws.upgrade().unwrap();
    tokio::spawn(async move {
        match fut.await {
            Ok(ws) => handle_connection(ws, state).await,
            Err(e) => tracing::error!("WS upgrade failed: {e}"),
        }
    });
    response
}

// ── helpers ──────────────────────────────────────

async fn send_event(tx: &mpsc::Sender<Vec<u8>>, event: &GatewayEvent) -> bool {
    match serde_json::to_vec(event) {
        Ok(data) => tx.send(data).await.is_ok(),
        Err(_) => false,
    }
}

/// Send event AND buffer it in the session for replay.
async fn dispatch(
    tx: &mpsc::Sender<Vec<u8>>,
    event: &GatewayEvent,
    session: &SessionHandle,
) -> bool {
    match serde_json::to_vec(event) {
        Ok(data) => {
            if let Some(seq) = event.s {
                session.push(seq, data.clone());
            }
            tx.send(data).await.is_ok()
        }
        Err(_) => false,
    }
}

fn noop_send(_frame: Frame<'_>) -> future::Ready<Result<(), WebSocketError>> {
    future::ready(Ok(()))
}

// ── connection entry point ───────────────────────

async fn handle_connection<S>(ws: fastwebsockets::WebSocket<S>, state: AppState)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (rx, tx) = ws.split(tokio::io::split);
    let mut rx = FragmentCollectorRead::new(rx);

    let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>(256);
    let writer = tokio::spawn(writer_task(tx, out_rx));

    // ── HELLO ──
    let hello = GatewayEvent {
        op: Opcode::Hello as u8,
        s: None,
        t: None,
        d: serde_json::json!({ "heartbeat_interval": HEARTBEAT_INTERVAL_MS }),
    };
    if !send_event(&out_tx, &hello).await {
        writer.abort();
        return;
    }

    // ── Wait for IDENTIFY or RESUME ──
    match wait_for_auth(&mut rx).await {
        Some(AuthCommand::Identify { token }) => {
            let claims = match auth::verify_token(&token) {
                Ok(c) => c,
                Err(_) => {
                    send_invalid_session(&out_tx, false).await;
                    drop(out_tx);
                    let _ = writer.await;
                    return;
                }
            };

            tracing::info!(user = %claims.username, "gateway IDENTIFY");

            let hub_id_num = claims.hub_id;
            let nats_subject = format!("hub.{hub_id_num}.channel.>");
            let (session_id, session) = state.sessions.create(claims, nats_subject);

            // Send READY (op=0 DISPATCH with t=READY)
            let ready = GatewayEvent {
                op: Opcode::Dispatch as u8,
                s: None,
                t: Some("READY".to_string()),
                d: serde_json::json!({
                    "session_id": session_id,
                    "user": {
                        // Stringified — see Claims doc for why.
                        "id": session.claims().sub.to_string(),
                        "username": session.claims().username,
                    },
                }),
            };
            if !send_event(&out_tx, &ready).await {
                drop(out_tx);
                writer.abort();
                return;
            }

            run_session(rx, out_tx, writer, state, session_id, session, 0).await;
        }

        Some(AuthCommand::Resume { session_id, seq }) => {
            let session = match state.sessions.get(&session_id) {
                Some(s) => s,
                None => {
                    tracing::debug!(%session_id, "RESUME: session not found");
                    send_invalid_session(&out_tx, false).await;
                    drop(out_tx);
                    let _ = writer.await;
                    return;
                }
            };

            // Try to replay buffered events
            let replay = match session.replay_from(seq) {
                Some(events) => events,
                None => {
                    tracing::debug!(%session_id, %seq, "RESUME: seq outside buffer");
                    send_invalid_session(&out_tx, false).await;
                    drop(out_tx);
                    let _ = writer.await;
                    return;
                }
            };

            session.mark_connected();
            let last_seq = session.last_seq();

            tracing::info!(
                user = %session.claims().username,
                %session_id,
                replayed = replay.len(),
                "gateway RESUMED"
            );

            // Replay buffered events
            for event_data in replay {
                if out_tx.send(event_data).await.is_err() {
                    drop(out_tx);
                    let _ = writer.await;
                    return;
                }
            }

            // Send RESUMED dispatch
            let resumed = GatewayEvent {
                op: Opcode::Dispatch as u8,
                s: None,
                t: Some("RESUMED".to_string()),
                d: serde_json::Value::Null,
            };
            if !send_event(&out_tx, &resumed).await {
                drop(out_tx);
                let _ = writer.await;
                return;
            }

            run_session(rx, out_tx, writer, state, session_id, session, last_seq).await;
        }

        None => {
            send_invalid_session(&out_tx, false).await;
            drop(out_tx);
            let _ = writer.await;
        }
    }
}

async fn send_invalid_session(tx: &mpsc::Sender<Vec<u8>>, resumable: bool) {
    let event = GatewayEvent {
        op: Opcode::InvalidSession as u8,
        s: None,
        t: None,
        d: serde_json::json!(resumable),
    };
    let _ = send_event(tx, &event).await;
}

// ── main session loop ────────────────────────────

async fn run_session<R: AsyncRead + Unpin>(
    mut rx: FragmentCollectorRead<R>,
    out_tx: mpsc::Sender<Vec<u8>>,
    writer: tokio::task::JoinHandle<()>,
    state: AppState,
    session_id: String,
    session: SessionHandle,
    initial_seq: u64,
) {
    // Subscribe to NATS
    let nats_subject = session.nats_subject();
    let mut nats_rx = match state.fanout.nats.subscribe(nats_subject).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("NATS subscribe failed: {e}");
            drop(out_tx);
            writer.abort();
            return;
        }
    };

    // NATS -> dispatch channel.
    //
    // The NATS subscription is hub-wide ("hub.{hub_id}.channel.>"), which means
    // we receive every channel's events, including ones the connected user has
    // no business reading (private text channels, other users' DMs). Filter
    // server-side via access::check before forwarding. Cache hits in Redis
    // make this dirt cheap on the hot path.
    let hub_id_for_filter = session.claims().hub_id;
    let user_id_for_filter = session.claims().sub;
    let state_for_filter = state.clone();
    let (dispatch_tx, mut dispatch_rx) = mpsc::channel::<GatewayEvent>(256);
    let nats_dispatch = dispatch_tx.clone();
    let nats_task = tokio::spawn(async move {
        while let Some(msg) = nats_rx.next().await {
            // Subject layout: "hub.{hub_id}.channel.{channel_id}.{event}".
            // Pull channel_id out for the ACL check.
            let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
            let event_type = match parts.last().copied() {
                Some("message") => events::MESSAGE_CREATE,
                Some("typing") => events::TYPING_START,
                Some("message_update") => events::MESSAGE_UPDATE,
                Some("message_delete") => events::MESSAGE_DELETE,
                Some("attachment_updated") => events::ATTACHMENT_UPDATED,
                Some("dm_call_invite") => events::DM_CALL_INVITE,
                Some("dm_call_decline") => events::DM_CALL_DECLINE,
                Some("dm_call_cancel") => events::DM_CALL_CANCEL,
                Some("dm_call_ended") => events::DM_CALL_ENDED,
                _ => continue,
            };
            let Some(channel_id) = parts.get(3).and_then(|s| s.parse::<i64>().ok()) else {
                tracing::warn!(subject = %msg.subject, "fanout: cannot parse channel_id, dropping");
                continue;
            };

            if !crate::access::check(
                &state_for_filter,
                hub_id_for_filter,
                channel_id,
                user_id_for_filter,
                crate::access::Action::Read,
            )
            .await
            {
                continue;
            }

            if let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&msg.payload) {
                let event = GatewayEvent {
                    op: Opcode::Dispatch as u8,
                    s: None,
                    t: Some(event_type.to_string()),
                    d: payload,
                };
                if nats_dispatch.send(event).await.is_err() {
                    break;
                }
            }
        }
    });

    let mut heartbeat_timer = tokio::time::interval(Duration::from_millis(HEARTBEAT_INTERVAL_MS));
    let mut last_heartbeat = tokio::time::Instant::now();
    let mut seq = initial_seq;
    let username = session.claims().username;

    metrics::gauge!("matehub_chat_active_ws_connections").increment(1.0);

    loop {
        let mut ctrl_send = noop_send;
        tokio::select! {
            // Outbound: dispatch events to client
            Some(mut event) = dispatch_rx.recv() => {
                seq += 1;
                event.s = Some(seq);
                if !dispatch(&out_tx, &event, &session).await {
                    break;
                }
            }

            // Inbound: client frames
            frame = rx.read_frame(&mut ctrl_send) => {
                match frame {
                    Ok(frame) => match frame.opcode {
                        OpCode::Text => {
                            let payload = frame.payload.as_ref();
                            if let Ok(text) = std::str::from_utf8(payload) {
                                handle_client_cmd(text, &out_tx, &mut last_heartbeat).await;
                            }
                        }
                        OpCode::Close => break,
                        _ => {}
                    },
                    Err(_) => break,
                }
            }

            // Heartbeat timeout
            _ = heartbeat_timer.tick() => {
                if last_heartbeat.elapsed() > Duration::from_millis(HEARTBEAT_TIMEOUT_MS) {
                    tracing::info!(%username, "heartbeat timeout");
                    break;
                }
            }
        }
    }

    // ── Cleanup ──
    nats_task.abort();
    session.mark_disconnected();
    metrics::gauge!("matehub_chat_active_ws_connections").decrement(1.0);
    drop(out_tx);
    let _ = writer.await;
    tracing::info!(%username, %session_id, "session disconnected (buffer retained for RESUME)");
}

// ── client commands ──────────────────────────────

async fn handle_client_cmd(
    text: &str,
    out_tx: &mpsc::Sender<Vec<u8>>,
    last_heartbeat: &mut tokio::time::Instant,
) {
    let cmd: GatewayCommand = match serde_json::from_str(text) {
        Ok(c) => c,
        Err(_) => return,
    };

    if cmd.op == Opcode::Heartbeat as u8 {
        *last_heartbeat = tokio::time::Instant::now();
        let ack = GatewayEvent {
            op: Opcode::HeartbeatAck as u8,
            s: None,
            t: None,
            d: serde_json::Value::Null,
        };
        let _ = send_event(out_tx, &ack).await;
    } else {
        tracing::debug!(op = cmd.op, "unhandled gateway command");
    }
}

// ── writer task ──────────────────────────────────

async fn writer_task<W: AsyncWrite + Unpin>(
    mut tx: WebSocketWrite<W>,
    mut rx: mpsc::Receiver<Vec<u8>>,
) {
    while let Some(data) = rx.recv().await {
        if tx
            .write_frame(Frame::text(Payload::Owned(data)))
            .await
            .is_err()
        {
            break;
        }
    }
    let _ = tx.write_frame(Frame::close(1000, b"")).await;
}

// ── auth handshake ───────────────────────────────

enum AuthCommand {
    Identify { token: String },
    Resume { session_id: String, seq: u64 },
}

/// Wait for IDENTIFY or RESUME (10s timeout).
async fn wait_for_auth<R: AsyncRead + Unpin>(
    rx: &mut FragmentCollectorRead<R>,
) -> Option<AuthCommand> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let mut ctrl = noop_send;
            let frame = rx.read_frame(&mut ctrl).await.ok()?;
            if frame.opcode == OpCode::Text {
                let text = std::str::from_utf8(frame.payload.as_ref()).ok()?;
                let cmd: GatewayCommand = serde_json::from_str(text).ok()?;

                if cmd.op == Opcode::Identify as u8 {
                    let token = cmd.d.get("token")?.as_str()?.to_string();
                    return Some(AuthCommand::Identify { token });
                }

                if cmd.op == Opcode::Resume as u8 {
                    let session_id = cmd.d.get("session_id")?.as_str()?.to_string();
                    let seq = cmd.d.get("seq")?.as_u64()?;
                    return Some(AuthCommand::Resume { session_id, seq });
                }
            }
        }
    })
    .await
    .ok()
    .flatten()
}
