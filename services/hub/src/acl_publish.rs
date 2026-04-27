//! ACL invalidation events.
//!
//! Whenever hub mutates anything that goes into chat's `access::check` cache —
//! membership, channel permissions, DM participants — we publish a NATS event
//! so chat (and any other consumer) can drop the relevant Redis keys without
//! waiting for the 30 s TTL.
//!
//! The publisher is a process-global `OnceLock` because plumbing an async-nats
//! client through every route's State{} would be a thirty-file diff for one
//! optional side-effect. NATS-down or pre-init = no-op.

use std::sync::OnceLock;

use serde::Serialize;

static PUBLISHER: OnceLock<Option<async_nats::Client>> = OnceLock::new();

/// Subject all consumers subscribe to.
pub const SUBJECT: &str = "acl.invalidate";

/// Wire up the global publisher. Call once at startup. `None` disables
/// publishing (NATS unavailable in dev).
pub fn init(client: Option<async_nats::Client>) {
    let _ = PUBLISHER.set(client);
}

#[derive(Debug, Serialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum InvalidatePayload {
    /// Drop ACL cache for everybody on this channel. Use after channel-perm
    /// edits or when a new DM is minted.
    Channel {
        #[serde(with = "matehub_common::serde_i64::as_string")]
        hub_id: i64,
        #[serde(with = "matehub_common::serde_i64::as_string")]
        channel_id: i64,
    },
    /// Drop every cached ACL entry for this user. Use after group-membership
    /// changes (any channel they touch could now allow/deny differently).
    User {
        #[serde(with = "matehub_common::serde_i64::as_string")]
        hub_id: i64,
        #[serde(with = "matehub_common::serde_i64::as_string")]
        user_id: i64,
    },
}

pub async fn invalidate_channel(hub_id: i64, channel_id: i64) {
    publish(InvalidatePayload::Channel { hub_id, channel_id }).await;
}

pub async fn invalidate_user(hub_id: i64, user_id: i64) {
    publish(InvalidatePayload::User { hub_id, user_id }).await;
}

async fn publish(payload: InvalidatePayload) {
    let Some(Some(client)) = PUBLISHER.get() else {
        return;
    };
    let Ok(data) = serde_json::to_vec(&payload) else {
        tracing::warn!("acl invalidate: payload serialization failed");
        return;
    };
    if let Err(e) = client.publish(SUBJECT, data.into()).await {
        tracing::warn!(error = %e, "acl invalidate publish failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_payload_round_trips() {
        let p = InvalidatePayload::Channel {
            hub_id: 1,
            channel_id: 9_223_372_036_854_775_000,
        };
        let json = serde_json::to_string(&p).unwrap();
        // tag+stringified ids are the load-bearing part.
        assert!(json.contains("\"scope\":\"channel\""));
        assert!(json.contains("\"channel_id\":\"9223372036854775000\""));
        assert!(json.contains("\"hub_id\":\"1\""));
    }

    #[test]
    fn user_payload_round_trips() {
        let p = InvalidatePayload::User {
            hub_id: 1,
            user_id: 1001,
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"scope\":\"user\""));
        assert!(json.contains("\"user_id\":\"1001\""));
    }
}
