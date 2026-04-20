use redis::AsyncCommands;
use uuid::Uuid;

const PRESENCE_TTL_SECS: u64 = 30;

pub type RedisPool = redis::aio::MultiplexedConnection;

pub async fn connect_redis() -> Option<RedisPool> {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
    match redis::Client::open(url.as_str()) {
        Ok(client) => match client.get_multiplexed_async_connection().await {
            Ok(conn) => {
                tracing::info!(%url, "Redis connected (presence)");
                Some(conn)
            }
            Err(e) => {
                tracing::warn!("Redis connection failed: {e}, presence disabled");
                None
            }
        },
        Err(e) => {
            tracing::warn!("Redis client error: {e}, presence disabled");
            None
        }
    }
}

fn presence_key(hub_id: Uuid, user_id: Uuid) -> String {
    format!("presence:{hub_id}:{user_id}")
}

/// Mark user as online. Returns a session_id to use in set_offline.
pub async fn set_online(conn: &mut RedisPool, hub_id: Uuid, user_id: Uuid) -> String {
    let session_id = Uuid::new_v4().to_string();
    let key = presence_key(hub_id, user_id);
    match conn.set_ex::<_, _, ()>(&key, &session_id, PRESENCE_TTL_SECS).await {
        Ok(()) => tracing::debug!(%key, %session_id, "presence SET OK"),
        Err(e) => tracing::error!(%key, "presence SET failed: {e}"),
    }
    session_id
}

/// Refresh presence -- re-SET with our session_id to reset TTL atomically
pub async fn refresh(conn: &mut RedisPool, hub_id: Uuid, user_id: Uuid, session_id: &str) {
    let key = presence_key(hub_id, user_id);
    let _: Result<(), _> = conn.set_ex(&key, session_id, PRESENCE_TTL_SECS).await;
}

/// Mark user as offline -- only if the session_id matches (don't kill newer sessions)
pub async fn set_offline(conn: &mut RedisPool, hub_id: Uuid, user_id: Uuid, session_id: &str) {
    let key = presence_key(hub_id, user_id);
    let current: Option<String> = conn.get(&key).await.unwrap_or(None);
    if current.as_deref() == Some(session_id) {
        let _: Result<(), _> = conn.del(&key).await;
        tracing::debug!(%key, %session_id, "presence DEL (our session)");
    } else {
        tracing::debug!(%key, %session_id, current = ?current, "presence DEL skipped (not our session)");
    }
}

/// Check if a single user is online
#[allow(dead_code)]
pub async fn is_online(conn: &mut RedisPool, hub_id: Uuid, user_id: Uuid) -> bool {
    let key = presence_key(hub_id, user_id);
    conn.exists(&key).await.unwrap_or(false)
}

// ── Voice occupancy ────────────────────────────────────────────────────────
// Which voice channel (if any) each user is currently connected to. Live
// state owned by the video service; hub caches it in Redis and rebroadcasts
// changes to presence-WS clients.

fn voice_occupancy_key(hub_id: Uuid) -> String {
    format!("voice_occupancy:{hub_id}")
}

/// Mark user as being in a voice channel. Idempotent.
pub async fn voice_occupancy_set(
    conn: &mut RedisPool,
    hub_id: Uuid,
    user_id: Uuid,
    channel_id: Uuid,
) {
    let key = voice_occupancy_key(hub_id);
    match conn
        .hset::<_, _, _, ()>(&key, user_id.to_string(), channel_id.to_string())
        .await
    {
        Ok(()) => tracing::debug!(%key, %user_id, %channel_id, "voice occupancy SET OK"),
        Err(e) => tracing::error!(%key, "voice occupancy SET failed: {e}"),
    }
}

/// Remove user from voice-channel tracking. Idempotent.
pub async fn voice_occupancy_clear(conn: &mut RedisPool, hub_id: Uuid, user_id: Uuid) {
    let key = voice_occupancy_key(hub_id);
    let _: Result<(), _> = conn.hdel(&key, user_id.to_string()).await;
    tracing::debug!(%key, %user_id, "voice occupancy CLEAR");
}

/// Read every user → channel mapping for a hub.
pub async fn voice_occupancy_snapshot(
    conn: &mut RedisPool,
    hub_id: Uuid,
) -> std::collections::HashMap<Uuid, Uuid> {
    let key = voice_occupancy_key(hub_id);
    let map: std::collections::HashMap<String, String> =
        conn.hgetall(&key).await.unwrap_or_default();
    map.into_iter()
        .filter_map(|(k, v)| {
            let uid = Uuid::parse_str(&k).ok()?;
            let cid = Uuid::parse_str(&v).ok()?;
            Some((uid, cid))
        })
        .collect()
}

/// Batch check: which of these user_ids are online
pub async fn get_online_set(
    conn: &mut RedisPool,
    hub_id: Uuid,
    user_ids: &[Uuid],
) -> std::collections::HashSet<Uuid> {
    let mut online = std::collections::HashSet::new();
    if user_ids.is_empty() {
        return online;
    }

    let mut pipe = redis::pipe();
    for uid in user_ids {
        pipe.exists(presence_key(hub_id, *uid));
    }

    let results: Vec<bool> = match pipe.query_async(conn).await {
        Ok(r) => r,
        Err(_) => return online,
    };

    for (uid, is_on) in user_ids.iter().zip(results) {
        if is_on {
            online.insert(*uid);
        }
    }
    online
}
