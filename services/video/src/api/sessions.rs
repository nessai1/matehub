use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::debug_capture::{MAX_BUNDLE_BYTES, participant_id_from_bundle};
use crate::state::{
    AppState, ChannelId, HubId, ParticipantInfoResponse, Session, SessionId, SessionInfoResponse,
    SessionResponse,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/sessions", post(create_session))
        .route("/v1/sessions/{session_id}", get(get_session))
        // ROOM_DEBUG bundle sink. Body-limited so a client (or a stray caller)
        // can't push arbitrarily large payloads; no-ops with 404 when capture
        // is off (see handler), so it's safe to leave mounted unconditionally.
        .route(
            "/v1/sessions/{session_id}/debug",
            post(upload_debug_bundle).layer(DefaultBodyLimit::max(MAX_BUNDLE_BYTES)),
        )
}

#[derive(Deserialize)]
struct CreateSessionRequest {
    /// Snowflake as string (JS `Number.MAX_SAFE_INTEGER` is smaller than the
    /// current Sonyflake range, so we ship IDs as strings to avoid silent
    /// precision loss in browser `JSON.parse`).
    #[serde(with = "matehub_common::serde_i64::as_string")]
    channel_id: ChannelId,
    /// Hub owning the channel. Kept on the session so we can tag voice-
    /// occupancy NATS events without a cross-service DB lookup.
    #[serde(with = "matehub_common::serde_i64::as_string")]
    hub_id: HubId,
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

    let debug_capture = state.debug_capture.is_some();

    // Idempotent: return existing session for this channel
    if let Some(&session_id) = inner.channel_to_session.get(&req.channel_id) {
        let resp = SessionResponse {
            session_id,
            ws_url: format!("{ws_base}/ws/{session_id}"),
            created: false,
            debug_capture,
        };
        return Ok((StatusCode::OK, Json(resp)));
    }

    // Create new session
    let session_id = Uuid::new_v4();
    let session = Session {
        id: session_id,
        channel_id: req.channel_id,
        hub_id: req.hub_id,
        participants: std::collections::HashMap::new(),
        created_at: chrono::Utc::now(),
    };

    inner.sessions.insert(session_id, session);
    inner.channel_to_session.insert(req.channel_id, session_id);
    metrics::counter!("matehub_video_session_creates_total").increment(1);

    tracing::info!(%session_id, channel_id = %req.channel_id, "session created");

    let resp = SessionResponse {
        session_id,
        ws_url: format!("{ws_base}/ws/{session_id}"),
        created: true,
        debug_capture,
    };
    Ok((StatusCode::CREATED, Json(resp)))
}

/// Persist a client's debug bundle for a call (ROOM_DEBUG only).
///
/// The body is the JSON snapshot the debug panel already assembles
/// (`{ userId, sessionId, participantId, timestamp, userAgent, diagnostics,
/// logs }`). We persist it verbatim under `room-debug/<session_id>/` keyed by
/// the participant + an upload sequence number, so the per-call directory
/// shows each client's timeline. Returns:
///   * 404 when capture is disabled (so clients stop trying),
///   * 204 on success — fire-and-forget, the writer thread does the I/O.
async fn upload_debug_bundle(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    body: Bytes,
) -> StatusCode {
    let Some(capture) = state.debug_capture.as_ref() else {
        return StatusCode::NOT_FOUND;
    };
    // Only accept bundles for sessions that actually exist: the route is
    // unauthenticated, so without this gate any caller could mint directories
    // and up-to-4MiB files under arbitrary UUIDs (and grow the capture's
    // per-(session, participant) seq map) for as long as ROOM_DEBUG is on.
    if !state.inner.lock().sessions.contains_key(&session_id) {
        return StatusCode::NOT_FOUND;
    }
    let participant = participant_id_from_bundle(&body).unwrap_or_else(|| "unknown".to_string());
    capture.record_client_bundle(session_id, &participant, body.to_vec());
    StatusCode::NO_CONTENT
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
