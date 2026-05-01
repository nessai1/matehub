use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct CreateTempUser {
    pub nickname: String,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub group_id: i64,
    /// TTL in seconds (e.g., 43200 = 12 hours)
    pub ttl_seconds: i64,
}

/// Public response from POST /temp-users — the inviter just needs the URL
/// and the deadline so they can copy/share/explain.
#[derive(Debug, Serialize)]
pub struct TempUserLink {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub user_id: i64,
    pub nickname: String,
    pub invite_url: String,
    pub expires_at: DateTime<Utc>,
}
