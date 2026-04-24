use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TempUser {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub id: i64,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub hub_id: i64,
    pub token: String,
    pub nickname: String,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub group_id: i64,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub created_by: i64,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub active_session: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateTempUser {
    pub nickname: String,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub group_id: i64,
    /// TTL in seconds (e.g., 43200 = 12 hours)
    pub ttl_seconds: i64,
}

/// What the public sees (no internal fields)
#[derive(Debug, Serialize)]
pub struct TempUserLink {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub id: i64,
    pub nickname: String,
    pub invite_url: String,
    pub expires_at: DateTime<Utc>,
}
