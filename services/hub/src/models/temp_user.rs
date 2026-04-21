use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TempUser {
    pub id: i64,
    pub hub_id: i64,
    pub token: String,
    pub nickname: String,
    pub group_id: i64,
    pub created_by: i64,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub active_session: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateTempUser {
    pub nickname: String,
    pub group_id: i64,
    /// TTL in seconds (e.g., 43200 = 12 hours)
    pub ttl_seconds: i64,
}

/// What the public sees (no internal fields)
#[derive(Debug, Serialize)]
pub struct TempUserLink {
    pub id: i64,
    pub nickname: String,
    pub invite_url: String,
    pub expires_at: DateTime<Utc>,
}
