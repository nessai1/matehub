use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TempUser {
    pub id: Uuid,
    pub hub_id: Uuid,
    pub token: String,
    pub nickname: String,
    pub group_id: Uuid,
    pub created_by: Uuid,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub active_session: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateTempUser {
    pub nickname: String,
    pub group_id: Uuid,
    /// TTL in seconds (e.g., 43200 = 12 hours)
    pub ttl_seconds: i64,
}

/// What the public sees (no internal fields)
#[derive(Debug, Serialize)]
pub struct TempUserLink {
    pub id: Uuid,
    pub nickname: String,
    pub invite_url: String,
    pub expires_at: DateTime<Utc>,
}
