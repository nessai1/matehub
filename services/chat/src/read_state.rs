use std::sync::Arc;

use redis::AsyncCommands;

use crate::data_service::DataService;

pub type RedisPool = redis::aio::MultiplexedConnection;

pub async fn connect_redis() -> Option<RedisPool> {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
    match redis::Client::open(url.as_str()) {
        Ok(client) => match client.get_multiplexed_async_connection().await {
            Ok(conn) => {
                tracing::info!(%url, "Redis connected (chat)");
                Some(conn)
            }
            Err(e) => {
                tracing::warn!("Redis connection failed: {e}");
                None
            }
        },
        Err(e) => {
            tracing::warn!("Redis client error: {e}");
            None
        }
    }
}

// ── Read State (write-through: Redis cache + ScyllaDB source of truth) ──

fn unread_key(user_id: &str, channel_id: i64) -> String {
    format!("unread:{user_id}:{channel_id}")
}

fn mention_key(user_id: &str, channel_id: i64) -> String {
    format!("mentions:{user_id}:{channel_id}")
}

/// Increment unread counter (Redis hot cache only -- ScyllaDB updated on mark_read)
pub async fn increment_unread(conn: &mut RedisPool, user_id: &str, channel_id: i64) {
    let _: Result<(), _> = conn.incr(unread_key(user_id, channel_id), 1i64).await;
}

/// Increment mention counter (Redis hot cache only)
pub async fn increment_mentions(conn: &mut RedisPool, user_id: &str, channel_id: i64) {
    let _: Result<(), _> = conn.incr(mention_key(user_id, channel_id), 1i64).await;
}

/// Mark channel as read. Write-through: clears Redis cache + persists to ScyllaDB.
pub async fn mark_read(
    conn: &mut RedisPool,
    data: &Arc<DataService>,
    user_id: &str,
    hub_id: i64,
    channel_id: i64,
    last_read_message_id: i64,
) {
    // Clear Redis cache
    let _: Result<(), _> = conn.del(unread_key(user_id, channel_id)).await;
    let _: Result<(), _> = conn.del(mention_key(user_id, channel_id)).await;

    // Persist to ScyllaDB (source of truth)
    if let Err(e) = data.mark_read(user_id, hub_id, channel_id, last_read_message_id).await {
        tracing::error!("ScyllaDB mark_read failed: {e}");
    }
}

/// Get unread + mention counts. Tries Redis first, falls back to ScyllaDB.
pub async fn get_unread(
    conn: &mut RedisPool,
    data: &Arc<DataService>,
    user_id: &str,
    hub_id: i64,
    channel_id: i64,
) -> (i64, i64) {
    // Try Redis hot cache
    let unread: Option<i64> = conn.get(unread_key(user_id, channel_id)).await.ok();
    let mentions: Option<i64> = conn.get(mention_key(user_id, channel_id)).await.ok();

    if let (Some(u), Some(m)) = (unread, mentions) {
        return (u, m);
    }

    // Cache miss: read from ScyllaDB
    match data.get_read_state(user_id, hub_id, channel_id).await {
        Ok(Some((_last_read, mention_count))) => {
            // We don't have unread count in ScyllaDB directly -- it stores last_read_message_id.
            // Unread count requires comparing against latest message, which is expensive.
            // Return mention_count from DB, unread=0 (conservative -- client will re-sync).
            (0, mention_count as i64)
        }
        _ => (0, 0),
    }
}

// ── Idempotency Cache (Redis only -- ephemeral) ──

fn idempotency_key(user_id: &str, client_id: &str) -> String {
    format!("idem:{user_id}:{client_id}")
}

pub async fn check_idempotency(
    conn: &mut RedisPool,
    user_id: &str,
    client_id: &str,
) -> Option<i64> {
    let key = idempotency_key(user_id, client_id);
    let val: Option<i64> = conn.get(&key).await.ok()?;
    val
}

pub async fn set_idempotency(
    conn: &mut RedisPool,
    user_id: &str,
    client_id: &str,
    message_id: i64,
) {
    let key = idempotency_key(user_id, client_id);
    let _: Result<(), _> = conn.set_ex(&key, message_id, 86400).await;
}

// ── Rate Limiting (Redis only -- ephemeral) ──────

fn rate_key(user_id: &str, channel_id: i64) -> String {
    format!("rate:msg:{user_id}:{channel_id}")
}

pub async fn check_rate_limit(
    conn: &mut RedisPool,
    user_id: &str,
    channel_id: i64,
    limit: i64,
    window_secs: u64,
) -> bool {
    let key = rate_key(user_id, channel_id);
    let count: i64 = conn.incr(&key, 1i64).await.unwrap_or(1);
    if count == 1 {
        let _: Result<(), _> = conn.expire(&key, window_secs as i64).await;
    }
    count <= limit
}
