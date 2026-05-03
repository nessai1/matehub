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

fn presence_key(hub_id: i64, user_id: i64) -> String {
    format!("presence:{hub_id}:{user_id}")
}

/// Mark user as online. Returns a session_id to use in set_offline.
/// The session_id is a random UUID — opaque, only used to distinguish concurrent
/// connections of the same user from each other.
pub async fn set_online(conn: &mut RedisPool, hub_id: i64, user_id: i64) -> String {
    let session_id = Uuid::new_v4().to_string();
    let key = presence_key(hub_id, user_id);
    match conn
        .set_ex::<_, _, ()>(&key, &session_id, PRESENCE_TTL_SECS)
        .await
    {
        Ok(()) => tracing::debug!(%key, %session_id, "presence SET OK"),
        Err(e) => tracing::error!(%key, "presence SET failed: {e}"),
    }
    session_id
}

/// Refresh presence -- re-SET with our session_id to reset TTL atomically
pub async fn refresh(conn: &mut RedisPool, hub_id: i64, user_id: i64, session_id: &str) {
    let key = presence_key(hub_id, user_id);
    let _: Result<(), _> = conn.set_ex(&key, session_id, PRESENCE_TTL_SECS).await;
}

/// Mark user as offline -- only if the session_id matches (don't kill newer sessions)
pub async fn set_offline(conn: &mut RedisPool, hub_id: i64, user_id: i64, session_id: &str) {
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
pub async fn is_online(conn: &mut RedisPool, hub_id: i64, user_id: i64) -> bool {
    let key = presence_key(hub_id, user_id);
    conn.exists(&key).await.unwrap_or(false)
}

// ── Voice occupancy ────────────────────────────────────────────────────────

fn voice_occupancy_key(hub_id: i64) -> String {
    format!("voice_occupancy:{hub_id}")
}

/// Mark user as being in a voice channel. Idempotent.
pub async fn voice_occupancy_set(conn: &mut RedisPool, hub_id: i64, user_id: i64, channel_id: i64) {
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
pub async fn voice_occupancy_clear(conn: &mut RedisPool, hub_id: i64, user_id: i64) {
    let key = voice_occupancy_key(hub_id);
    let _: Result<(), _> = conn.hdel(&key, user_id.to_string()).await;
    tracing::debug!(%key, %user_id, "voice occupancy CLEAR");
}

/// Read every user → channel mapping for a hub.
pub async fn voice_occupancy_snapshot(
    conn: &mut RedisPool,
    hub_id: i64,
) -> std::collections::HashMap<i64, i64> {
    let key = voice_occupancy_key(hub_id);
    let map: std::collections::HashMap<String, String> =
        conn.hgetall(&key).await.unwrap_or_default();
    map.into_iter()
        .filter_map(|(k, v)| {
            let uid = k.parse::<i64>().ok()?;
            let cid = v.parse::<i64>().ok()?;
            Some((uid, cid))
        })
        .collect()
}

// ── Voice mute state ──────────────────────────────────────────
// Stored as `hub:<hub_id>:voice_mute` hash, keys `<user_id>:<kind>`,
// values "1" / "0". Cleared en-bloc when the user leaves voice (the
// occupancy bus calls `voice_mute_clear` to drop both audio + video
// entries together).

fn voice_mute_key(hub_id: i64) -> String {
    format!("hub:{hub_id}:voice_mute")
}

fn voice_mute_field(user_id: i64, kind: &str) -> String {
    format!("{user_id}:{kind}")
}

pub async fn voice_mute_set(
    conn: &mut RedisPool,
    hub_id: i64,
    user_id: i64,
    kind: &str,
    muted: bool,
) {
    let key = voice_mute_key(hub_id);
    let field = voice_mute_field(user_id, kind);
    let val = if muted { "1" } else { "0" };
    if let Err(e) = conn.hset::<_, _, _, ()>(&key, &field, val).await {
        tracing::error!(%key, %field, "voice mute SET failed: {e}");
    }
}

/// Remove all mute state for this user (both kinds). Called when the user
/// leaves voice altogether — occupancy bus invokes this.
pub async fn voice_mute_clear(conn: &mut RedisPool, hub_id: i64, user_id: i64) {
    let key = voice_mute_key(hub_id);
    let _: Result<(), _> = conn
        .hdel(
            &key,
            &[
                voice_mute_field(user_id, "audio"),
                voice_mute_field(user_id, "video"),
            ],
        )
        .await;
}

/// Snapshot returns map of user_id → (audio_muted, video_muted). Default
/// for missing entries is "true" (i.e. muted) — matches video service's
/// Participant initial state, so a member appearing in occupancy without
/// mute entries reads as both muted.
pub async fn voice_mute_snapshot(
    conn: &mut RedisPool,
    hub_id: i64,
) -> std::collections::HashMap<i64, (bool, bool)> {
    let key = voice_mute_key(hub_id);
    let map: std::collections::HashMap<String, String> =
        conn.hgetall(&key).await.unwrap_or_default();
    let mut out: std::collections::HashMap<i64, (bool, bool)> = std::collections::HashMap::new();
    for (k, v) in map {
        let Some((uid_s, kind)) = k.split_once(':') else {
            continue;
        };
        let Ok(uid) = uid_s.parse::<i64>() else {
            continue;
        };
        let muted = v == "1";
        let entry = out.entry(uid).or_insert((true, true));
        match kind {
            "audio" => entry.0 = muted,
            "video" => entry.1 = muted,
            _ => {}
        }
    }
    out
}

/// Batch check: which of these user_ids are online
pub async fn get_online_set(
    conn: &mut RedisPool,
    hub_id: i64,
    user_ids: &[i64],
) -> std::collections::HashSet<i64> {
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
