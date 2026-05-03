use axum::{
    Router,
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::get,
};
use futures_util::StreamExt;
use serde::Deserialize;
use sqlx::PgPool;
use tokio::sync::broadcast;

use crate::auth;
use crate::presence::{self, RedisPool};

/// One event on the presence broadcast bus, addressed to a specific hub.
#[derive(Clone, Debug)]
pub struct PresenceEvent {
    pub hub_id: i64,
    /// Already-serialized JSON payload that gets forwarded to WS clients
    /// verbatim. We carry it as a string so every subscriber doesn't have to
    /// re-serialize.
    pub payload: String,
}

#[derive(Clone)]
pub struct PresenceState {
    pub pool: PgPool,
    pub redis: Option<RedisPool>,
    pub events: broadcast::Sender<PresenceEvent>,
}

pub fn routes(state: PresenceState) -> Router {
    Router::new()
        .route("/ws/presence/{hub_id}", get(ws_upgrade))
        .with_state(state)
}

#[derive(Deserialize)]
struct WsQuery {
    token: String,
}

async fn ws_upgrade(
    State(state): State<PresenceState>,
    Path(hub_id): Path<i64>,
    Query(query): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let claims = match validate_token(&query.token, hub_id) {
        Some(c) => c,
        None => return axum::http::StatusCode::UNAUTHORIZED.into_response(),
    };

    ws.on_upgrade(move |socket| handle_presence(socket, state, hub_id, claims.sub))
}

fn validate_token(token: &str, hub_id: i64) -> Option<auth::Claims> {
    // Dev token shortcut (`dev-{username}-token`). No signature, just routed
    // through the seed user map so local frontend work bypasses the real login.
    if token.starts_with("dev-") && token.ends_with("-token") {
        let username = token.strip_prefix("dev-")?.strip_suffix("-token")?;
        let user_id = match username {
            "alice" => crate::db::seed::DEV_USER_ALICE,
            "bob" => crate::db::seed::DEV_USER_BOB,
            "charlie" => crate::db::seed::DEV_USER_CHARLIE,
            _ => return None,
        };
        return Some(auth::Claims {
            sub: user_id,
            username: username.to_string(),
            user_type: "permanent".into(),
            hub_id,
            groups: vec![],
            iat: chrono::Utc::now().timestamp(),
            exp: chrono::Utc::now().timestamp() + 86400,
        });
    }

    let claims = auth::verify_token(token).ok()?;
    if claims.hub_id != hub_id {
        return None;
    }
    Some(claims)
}

async fn handle_presence(socket: WebSocket, state: PresenceState, hub_id: i64, user_id: i64) {
    use futures_util::SinkExt;

    let session_id = if let Some(mut redis) = state.redis.clone() {
        let sid = presence::set_online(&mut redis, hub_id, user_id).await;
        tracing::info!(%hub_id, %user_id, %sid, "presence connected");
        Some(sid)
    } else {
        tracing::info!(%hub_id, %user_id, "presence connected (no redis)");
        None
    };

    let mut events = state.events.subscribe();

    let (mut ws_sender, mut ws_receiver) = socket.split();

    let mut heartbeat_interval = tokio::time::interval(std::time::Duration::from_secs(15));

    loop {
        tokio::select! {
            msg = ws_receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(_) | Message::Ping(_) | Message::Pong(_))) => {
                        if let (Some(mut redis), Some(sid)) = (state.redis.clone(), session_id.as_deref()) {
                            presence::refresh(&mut redis, hub_id, user_id, sid).await;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            event = events.recv() => {
                match event {
                    Ok(ev) if ev.hub_id == hub_id => {
                        if ws_sender.send(Message::Text(ev.payload.into())).await.is_err() {
                            break;
                        }
                    }
                    Ok(_) => {/* event for a different hub — skip */}
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(%hub_id, %user_id, dropped = n, "presence WS lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = heartbeat_interval.tick() => {
                if ws_sender.send(Message::Ping(vec![].into())).await.is_err() {
                    break;
                }
            }
        }
    }

    tracing::info!(%hub_id, %user_id, "presence disconnected");

    if let (Some(mut redis), Some(sid)) = (state.redis.clone(), session_id.as_deref()) {
        presence::set_offline(&mut redis, hub_id, user_id, sid).await;
    }

    let _ = sqlx::query(
        "UPDATE hub_members SET last_seen_at = now() WHERE hub_id = $1 AND user_id = $2",
    )
    .bind(hub_id)
    .bind(user_id)
    .execute(&state.pool)
    .await;
}
