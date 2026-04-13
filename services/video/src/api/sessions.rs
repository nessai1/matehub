use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::state::{
    AppState, ChannelId, ParticipantInfoResponse, Session, SessionId, SessionInfoResponse,
    SessionResponse,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/sessions", post(create_session))
        .route("/v1/sessions/{session_id}", get(get_session))
}

#[derive(Deserialize)]
struct CreateSessionRequest {
    channel_id: ChannelId,
}

fn ws_base_url(headers: &HeaderMap) -> String {
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost:4000");
    format!("ws://{host}")
}

async fn create_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<SessionResponse>), StatusCode> {
    let mut inner = state.inner.lock();
    let ws_base = ws_base_url(&headers);

    // Idempotent: return existing session for this channel
    if let Some(&session_id) = inner.channel_to_session.get(&req.channel_id) {
        let resp = SessionResponse {
            session_id,
            ws_url: format!("{ws_base}/ws/{session_id}"),
            created: false,
        };
        return Ok((StatusCode::OK, Json(resp)));
    }

    // Create new session
    let session_id = Uuid::new_v4();
    let session = Session {
        id: session_id,
        channel_id: req.channel_id,
        participants: std::collections::HashMap::new(),
        created_at: chrono::Utc::now(),
    };

    inner.sessions.insert(session_id, session);
    inner.channel_to_session.insert(req.channel_id, session_id);

    tracing::info!(%session_id, channel_id = %req.channel_id, "session created");

    let resp = SessionResponse {
        session_id,
        ws_url: format!("{ws_base}/ws/{session_id}"),
        created: true,
    };
    Ok((StatusCode::CREATED, Json(resp)))
}

async fn get_session(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
) -> Result<Json<SessionInfoResponse>, StatusCode> {
    let inner = state.inner.lock();
    let session = inner
        .sessions
        .get(&session_id)
        .ok_or(StatusCode::NOT_FOUND)?;

    let participants = session
        .participants
        .values()
        .map(|p| ParticipantInfoResponse {
            id: p.id,
            user_id: p.user_id.clone(),
            state: p.state,
        })
        .collect();

    Ok(Json(SessionInfoResponse {
        session_id: session.id,
        channel_id: session.channel_id,
        participants,
        created_at: session.created_at,
    }))
}
