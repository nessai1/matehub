//! NATS → Redis + presence-WS bridge for hub-wide mic/camera mute state.
//!
//! Peers in the same SFU session see each other's mute toggles via
//! `participant_muted` on the video service WS. But the sidebar shows
//! participants from OTHER voice channels too — and they need the same
//! mic-icon. This bus subscribes to `voice.mute` (published by the video
//! service on every `mute_changed`), persists in Redis (so new tabs see
//! the current state on first members-full fetch), and broadcasts a
//! presence-WS event so connected clients update their store live.
//!
//! Mirrors `voice_occupancy_bus.rs` — same structure, different subject.
//! Kept as separate files because the payloads differ (mute is a
//! per-kind toggle, occupancy is a single scalar).
//!
//! # Wire format
//!
//! Inbound (from video service):
//! ```json
//! { "hub_id": "1", "user_id": "1001", "kind": "audio", "muted": true }
//! ```
//!
//! Outbound (to presence WS clients):
//! ```json
//! { "type": "voice_mute", "user_id": "1001", "kind": "audio", "muted": true }
//! ```
//! IDs are decimal strings end-to-end (Snowflake > 2^53).

use async_nats::Subscriber;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::api::presence_ws::PresenceEvent;
use crate::presence::{self, RedisPool};

pub const SUBJECT: &str = "voice.mute";

#[derive(Debug, Clone, Deserialize)]
pub struct MuteEvent {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub hub_id: i64,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub user_id: i64,
    /// "audio" or "video". Anything else is ignored at the bus.
    pub kind: String,
    pub muted: bool,
}

#[derive(Debug, Serialize)]
struct WireEvent<'a> {
    r#type: &'a str,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    user_id: i64,
    kind: &'a str,
    muted: bool,
}

pub async fn spawn(
    nats_url: &str,
    redis: Option<RedisPool>,
    events: broadcast::Sender<PresenceEvent>,
) {
    let client = match async_nats::connect(nats_url).await {
        Ok(c) => {
            tracing::info!(%nats_url, "NATS connected (voice mute)");
            c
        }
        Err(e) => {
            tracing::warn!(%nats_url, error = %e, "NATS connect failed — voice mute disabled");
            return;
        }
    };

    let sub = match client.subscribe(SUBJECT.to_string()).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "NATS subscribe failed (voice.mute)");
            return;
        }
    };

    tokio::spawn(run(sub, redis, events));
}

async fn run(
    mut sub: Subscriber,
    redis: Option<RedisPool>,
    events: broadcast::Sender<PresenceEvent>,
) {
    while let Some(msg) = sub.next().await {
        let ev: MuteEvent = match serde_json::from_slice(&msg.payload) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "bad voice.mute payload");
                continue;
            }
        };
        if ev.kind != "audio" && ev.kind != "video" {
            tracing::warn!(kind = %ev.kind, "unknown mute kind");
            continue;
        }

        if let Some(mut conn) = redis.clone() {
            presence::voice_mute_set(&mut conn, ev.hub_id, ev.user_id, &ev.kind, ev.muted).await;
        }

        let payload = serde_json::to_string(&WireEvent {
            r#type: "voice_mute",
            user_id: ev.user_id,
            kind: &ev.kind,
            muted: ev.muted,
        })
        .unwrap_or_default();

        let _ = events.send(PresenceEvent {
            hub_id: ev.hub_id,
            payload,
        });
    }

    tracing::warn!("voice mute NATS stream ended");
}
