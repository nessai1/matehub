//! DM ringing endpoints.
//!
//! Three thin POST routes that just publish a NATS event onto the per-channel
//! subject — the gateway fan-out path then forwards them to anyone with READ
//! access on the DM channel (i.e., its two participants).
//!
//! No persistence: a missed call is a tree falling in an empty forest. If we
//! ever want a "missed-call" entry in the DM transcript, that's a separate
//! ScyllaDB write done by the caller side after a timeout.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use serde::{Deserialize, Serialize};

use crate::access::{self, Action};
use crate::api::AppState;
use crate::auth::AuthUser;

/// Synthetic author id used for system events (call lifecycle, etc.). Renders
/// as a centered grey line on the frontend. Stays constant so message-list
/// virtualization can dedupe and member-lookup can short-circuit.
const SYSTEM_AUTHOR_ID: &str = "system";
const SYSTEM_AUTHOR_TYPE: &str = "system";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/dms/{channel_id}/call/start", post(start))
        .route("/v1/dms/{channel_id}/call/decline", post(decline))
        .route("/v1/dms/{channel_id}/call/cancel", post(cancel))
        .route("/v1/dms/{channel_id}/call/end", post(end))
}

#[derive(Debug, Serialize)]
struct CallEventPayload {
    /// Whoever generated the event — recipient/inviter pair the UI uses to
    /// figure out which side it's on.
    from_user_id: String,
    channel_id: String,
    /// Optional reason marker (decline: "declined" | "timeout").
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct CallEndedPayload {
    from_user_id: String,
    channel_id: String,
    duration_secs: u32,
}

#[derive(Debug, Deserialize, Default)]
struct EmptyBody {}

#[derive(Debug, Deserialize, Default)]
struct DeclineRequest {
    /// "declined" (manual hang-up) or "timeout" (ringing bailed). Drives the
    /// system-message text. Anything else is treated as "declined".
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EndRequest {
    duration_secs: u32,
}

/// Persist + fan out a system message in the DM channel.
///
/// The text uses `$1` as a user placeholder (replaced on the frontend by the
/// `mentions[0]` user's display name) — keeps the chat service out of the
/// hub user table for a leaf feature.
async fn write_system_message(
    state: &AppState,
    hub_id: i64,
    channel_id: i64,
    content: &str,
    mentions: Vec<String>,
) {
    let msg = match state
        .data
        .write_message(
            hub_id,
            channel_id,
            SYSTEM_AUTHOR_ID,
            SYSTEM_AUTHOR_TYPE,
            content,
            mentions,
            vec![],
            false,
            vec![],
            None,
            None,
        )
        .await
    {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, hub_id, channel_id, "dm_calls: system message write failed");
            return;
        }
    };
    state.fanout.publish_message(&msg).await;
}

fn fmt_duration(secs: u32) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{m}m {s}s")
}

async fn publish_call_event(
    state: &AppState,
    hub_id: i64,
    channel_id: i64,
    from_user_id: i64,
    event_subject_suffix: &str,
    reason: Option<&str>,
) {
    let payload = serde_json::to_value(CallEventPayload {
        from_user_id: from_user_id.to_string(),
        channel_id: channel_id.to_string(),
        reason: reason.map(str::to_string),
    })
    .unwrap_or(serde_json::Value::Null);
    state
        .fanout
        .publish_event(hub_id, channel_id, event_subject_suffix, &payload)
        .await;
}

async fn publish_call_ended(
    state: &AppState,
    hub_id: i64,
    channel_id: i64,
    from_user_id: i64,
    duration_secs: u32,
) {
    let payload = serde_json::to_value(CallEndedPayload {
        from_user_id: from_user_id.to_string(),
        channel_id: channel_id.to_string(),
        duration_secs,
    })
    .unwrap_or(serde_json::Value::Null);
    state
        .fanout
        .publish_event(hub_id, channel_id, "dm_call_ended", &payload)
        .await;
}

// ── start ───────────────────────────────────────────

async fn start(
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    auth: AuthUser,
    Json(_): Json<EmptyBody>,
) -> Result<StatusCode, StatusCode> {
    let hub_id = auth.0.hub_id;
    if !access::check(&state, hub_id, channel_id, auth.0.sub, Action::Write).await {
        return Err(StatusCode::FORBIDDEN);
    }
    publish_call_event(
        &state,
        hub_id,
        channel_id,
        auth.0.sub,
        "dm_call_invite",
        None,
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// ── decline ─────────────────────────────────────────

async fn decline(
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<DeclineRequest>,
) -> Result<StatusCode, StatusCode> {
    let hub_id = auth.0.hub_id;
    // Read is enough — declining doesn't write anything; the event just
    // travels back over fan-out to the inviter's WS.
    if !access::check(&state, hub_id, channel_id, auth.0.sub, Action::Read).await {
        return Err(StatusCode::FORBIDDEN);
    }

    let reason = match body.reason.as_deref() {
        Some("timeout") => "timeout",
        _ => "declined",
    };

    publish_call_event(
        &state,
        hub_id,
        channel_id,
        auth.0.sub,
        "dm_call_decline",
        Some(reason),
    )
    .await;

    let template = match reason {
        "timeout" => "$1 didn't answer the call",
        _ => "$1 declined the call",
    };
    write_system_message(
        &state,
        hub_id,
        channel_id,
        template,
        vec![auth.0.sub.to_string()],
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

// ── cancel ──────────────────────────────────────────

async fn cancel(
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    auth: AuthUser,
    Json(_): Json<EmptyBody>,
) -> Result<StatusCode, StatusCode> {
    let hub_id = auth.0.hub_id;
    if !access::check(&state, hub_id, channel_id, auth.0.sub, Action::Read).await {
        return Err(StatusCode::FORBIDDEN);
    }
    // Cancel is the inviter's own bail-out before the recipient picked up.
    // No system message — the recipient will see the modal close and that's
    // it. (If we ever want a "missed call from X" entry we'd write it here.)
    publish_call_event(
        &state,
        hub_id,
        channel_id,
        auth.0.sub,
        "dm_call_cancel",
        None,
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// ── end ─────────────────────────────────────────────

async fn end(
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<EndRequest>,
) -> Result<StatusCode, StatusCode> {
    let hub_id = auth.0.hub_id;
    if !access::check(&state, hub_id, channel_id, auth.0.sub, Action::Read).await {
        return Err(StatusCode::FORBIDDEN);
    }
    publish_call_ended(&state, hub_id, channel_id, auth.0.sub, body.duration_secs).await;
    let content = format!("Call ended ({})", fmt_duration(body.duration_secs));
    write_system_message(&state, hub_id, channel_id, &content, vec![]).await;
    Ok(StatusCode::NO_CONTENT)
}

// ── tests ──────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_stringifies_ids() {
        let payload = CallEventPayload {
            from_user_id: 1001i64.to_string(),
            channel_id: 9_223_372_036_854_775_000i64.to_string(),
            reason: None,
        };
        let json = serde_json::to_string(&payload).unwrap();
        // Both ids land as JSON strings — large channel_ids would round on
        // the JS side as `Number`s, so this is load-bearing.
        assert!(json.contains("\"from_user_id\":\"1001\""));
        assert!(json.contains("\"channel_id\":\"9223372036854775000\""));
        // No `reason` field when None — kept off the wire.
        assert!(!json.contains("\"reason\""));
    }

    #[test]
    fn payload_includes_reason_when_set() {
        let payload = CallEventPayload {
            from_user_id: "1001".into(),
            channel_id: "42".into(),
            reason: Some("timeout".into()),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"reason\":\"timeout\""));
    }

    #[test]
    fn fmt_duration_handles_minutes_and_seconds() {
        assert_eq!(fmt_duration(0), "0m 0s");
        assert_eq!(fmt_duration(45), "0m 45s");
        assert_eq!(fmt_duration(132), "2m 12s");
        assert_eq!(fmt_duration(3661), "61m 1s");
    }
}
