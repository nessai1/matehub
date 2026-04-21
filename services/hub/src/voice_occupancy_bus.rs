//! NATS → Redis + presence-WS bridge for "who is in which voice channel".
//!
//! The video service is the source of truth: when a participant joins or
//! leaves a session it publishes one NATS message to `voice.occupancy`. This
//! module subscribes to that subject, updates the Redis hash that
//! `GET /v1/hubs/<id>/members-full` reads, and fans the change out to every
//! connected presence-WS client of the same hub.

use async_nats::Subscriber;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::api::presence_ws::PresenceEvent;
use crate::presence::{self, RedisPool};

pub const SUBJECT: &str = "voice.occupancy";

/// Payload published by the video service.
#[derive(Debug, Clone, Deserialize)]
pub struct OccupancyEvent {
    pub hub_id: i64,
    pub user_id: i64,
    /// `None` when the user leaves.
    pub channel_id: Option<i64>,
}

/// Wire format sent to WS subscribers.
#[derive(Debug, Serialize)]
struct WireEvent<'a> {
    r#type: &'a str,
    user_id: i64,
    channel_id: Option<i64>,
}

pub async fn spawn(
    nats_url: &str,
    redis: Option<RedisPool>,
    events: broadcast::Sender<PresenceEvent>,
) {
    let client = match async_nats::connect(nats_url).await {
        Ok(c) => {
            tracing::info!(%nats_url, "NATS connected (voice occupancy)");
            c
        }
        Err(e) => {
            tracing::warn!(%nats_url, error = %e, "NATS connect failed — voice occupancy disabled");
            return;
        }
    };

    let sub = match client.subscribe(SUBJECT.to_string()).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "NATS subscribe failed");
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
        let ev: OccupancyEvent = match serde_json::from_slice(&msg.payload) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "bad voice.occupancy payload");
                continue;
            }
        };

        if let Some(mut conn) = redis.clone() {
            match ev.channel_id {
                Some(cid) => {
                    presence::voice_occupancy_set(&mut conn, ev.hub_id, ev.user_id, cid).await;
                }
                None => {
                    presence::voice_occupancy_clear(&mut conn, ev.hub_id, ev.user_id).await;
                }
            }
        }

        let payload = serde_json::to_string(&WireEvent {
            r#type: "voice_occupancy",
            user_id: ev.user_id,
            channel_id: ev.channel_id,
        })
        .unwrap_or_default();

        let _ = events.send(PresenceEvent {
            hub_id: ev.hub_id,
            payload,
        });
    }

    tracing::warn!("voice occupancy NATS stream ended");
}
