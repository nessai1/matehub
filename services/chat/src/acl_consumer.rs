//! Consumer for `acl.invalidate` NATS events emitted by hub.
//!
//! On each message we sweep `acl:*` keys in Redis matching the affected
//! scope. The cache is small (one entry per (hub, channel, user, action)) and
//! invalidations are rare — SCAN+DEL is fine. For a hot scope with no keys
//! matched, SCAN returns immediately, so the overhead is bounded.

use redis::AsyncCommands;
use serde::Deserialize;

use crate::read_state::RedisPool;

const SUBJECT: &str = "acl.invalidate";

#[derive(Debug, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
enum InvalidatePayload {
    Channel {
        #[serde(deserialize_with = "matehub_common::serde_i64::as_string::deserialize")]
        hub_id: i64,
        #[serde(deserialize_with = "matehub_common::serde_i64::as_string::deserialize")]
        channel_id: i64,
    },
    User {
        #[serde(deserialize_with = "matehub_common::serde_i64::as_string::deserialize")]
        hub_id: i64,
        #[serde(deserialize_with = "matehub_common::serde_i64::as_string::deserialize")]
        user_id: i64,
    },
}

/// Spawn a background task that subscribes to `acl.invalidate` and applies
/// each event to Redis. Drops cleanly when NATS goes away.
pub fn spawn(nats: async_nats::Client, redis: Option<RedisPool>) {
    let Some(redis) = redis else {
        tracing::warn!("acl_consumer: Redis unavailable, ACL events will be ignored");
        return;
    };
    tokio::spawn(async move {
        let mut subscriber = match nats.subscribe(SUBJECT).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "acl_consumer: subscribe failed");
                return;
            }
        };
        tracing::info!(subject = SUBJECT, "acl_consumer running");
        use futures_util::StreamExt;
        while let Some(msg) = subscriber.next().await {
            let payload: InvalidatePayload = match serde_json::from_slice(&msg.payload) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = %e, "acl_consumer: malformed payload, dropping");
                    continue;
                }
            };
            let mut conn = redis.clone();
            apply(&mut conn, &payload).await;
        }
        tracing::warn!("acl_consumer: NATS subscription ended");
    });
}

async fn apply(conn: &mut RedisPool, payload: &InvalidatePayload) {
    let pattern = match payload {
        InvalidatePayload::Channel { hub_id, channel_id } => {
            format!("acl:{hub_id}:{channel_id}:*")
        }
        InvalidatePayload::User { hub_id, user_id } => {
            // Sandwich the user_id between hub and action so a substring
            // match on a different field can't false-positive.
            format!("acl:{hub_id}:*:{user_id}:*")
        }
    };

    let keys = match collect_matching(conn, &pattern).await {
        Ok(k) => k,
        Err(e) => {
            tracing::warn!(error = %e, %pattern, "acl_consumer: SCAN failed");
            return;
        }
    };

    if keys.is_empty() {
        tracing::debug!(%pattern, "acl_consumer: no keys matched");
        return;
    }

    let count = keys.len();
    if let Err(e) = conn.del::<_, ()>(keys).await {
        tracing::warn!(error = %e, "acl_consumer: DEL failed");
        return;
    }
    tracing::info!(%pattern, deleted = count, "ACL cache invalidated");
}

async fn collect_matching(conn: &mut RedisPool, pattern: &str) -> redis::RedisResult<Vec<String>> {
    let mut cursor: u64 = 0;
    let mut out = Vec::new();
    loop {
        let (next, batch): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(pattern)
            .arg("COUNT")
            .arg(200)
            .query_async(conn)
            .await?;
        out.extend(batch);
        if next == 0 {
            break;
        }
        cursor = next;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_payload_deserializes_string_ids() {
        let json = r#"{"scope":"channel","hub_id":"1","channel_id":"42"}"#;
        let p: InvalidatePayload = serde_json::from_str(json).unwrap();
        match p {
            InvalidatePayload::Channel { hub_id, channel_id } => {
                assert_eq!(hub_id, 1);
                assert_eq!(channel_id, 42);
            }
            _ => panic!("wrong scope"),
        }
    }

    #[test]
    fn user_payload_deserializes() {
        let json = r#"{"scope":"user","hub_id":"1","user_id":"1001"}"#;
        let p: InvalidatePayload = serde_json::from_str(json).unwrap();
        match p {
            InvalidatePayload::User { user_id, .. } => assert_eq!(user_id, 1001),
            _ => panic!("wrong scope"),
        }
    }
}
