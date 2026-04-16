use redis::AsyncCommands;

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

// ── Read State (unread counters) ────────────────

fn unread_key(user_id: &str, channel_id: i64) -> String {
    format!("unread:{user_id}:{channel_id}")
}

fn mention_key(user_id: &str, channel_id: i64) -> String {
    format!("mentions:{user_id}:{channel_id}")
}

/// Increment unread counter for a user in a channel
pub async fn increment_unread(conn: &mut RedisPool, user_id: &str, channel_id: i64) {
    let _: Result<(), _> = conn.incr(unread_key(user_id, channel_id), 1i64).await;
}

/// Increment mention counter
pub async fn increment_mentions(conn: &mut RedisPool, user_id: &str, channel_id: i64) {
    let _: Result<(), _> = conn.incr(mention_key(user_id, channel_id), 1i64).await;
}

/// Mark channel as read (reset counters)
pub async fn mark_read(conn: &mut RedisPool, user_id: &str, channel_id: i64) {
    let _: Result<(), _> = conn.del(unread_key(user_id, channel_id)).await;
    let _: Result<(), _> = conn.del(mention_key(user_id, channel_id)).await;
}

/// Get unread + mention counts for a user's channel
pub async fn get_unread(conn: &mut RedisPool, user_id: &str, channel_id: i64) -> (i64, i64) {
    let unread: i64 = conn.get(unread_key(user_id, channel_id)).await.unwrap_or(0);
    let mentions: i64 = conn.get(mention_key(user_id, channel_id)).await.unwrap_or(0);
    (unread, mentions)
}

// ── Idempotency Cache ───────────────────────────

fn idempotency_key(user_id: &str, client_id: &str) -> String {
    format!("idem:{user_id}:{client_id}")
}

/// Check if a message with this client_id was already processed.
/// Returns the stored message_id if duplicate, None if new.
pub async fn check_idempotency(
    conn: &mut RedisPool,
    user_id: &str,
    client_id: &str,
) -> Option<i64> {
    let key = idempotency_key(user_id, client_id);
    let val: Option<i64> = conn.get(&key).await.ok()?;
    val
}

/// Store idempotency result (TTL 24h)
pub async fn set_idempotency(
    conn: &mut RedisPool,
    user_id: &str,
    client_id: &str,
    message_id: i64,
) {
    let key = idempotency_key(user_id, client_id);
    let _: Result<(), _> = conn.set_ex(&key, message_id, 86400).await;
}

// ── Rate Limiting ───────────────────────────────

fn rate_key(user_id: &str, channel_id: i64) -> String {
    format!("rate:msg:{user_id}:{channel_id}")
}

/// Check rate limit: max `limit` messages per `window_secs` per user per channel.
/// Returns true if allowed, false if rate limited.
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
        // First message in window -- set TTL
        let _: Result<(), _> = conn.expire(&key, window_secs as i64).await;
    }
    count <= limit
}
