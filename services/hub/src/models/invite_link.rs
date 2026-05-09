use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct CreateInviteLinkRequest {
    pub expires_at: DateTime<Utc>,
    /// NULL = unlimited.
    pub max_uses: Option<i32>,
    /// NULL = everyone group only.
    #[serde(with = "matehub_common::serde_i64::option_as_string", default)]
    pub group_id: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct InviteLinkResponse {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub id: i64,
    pub token: String,
    pub invite_url: String,
    pub expires_at: DateTime<Utc>,
    pub max_uses: Option<i32>,
    pub uses_count: i32,
    #[serde(with = "matehub_common::serde_i64::option_as_string", default)]
    pub group_id: Option<i64>,
}

/// Public preview — what `/signup/{token}` shows before the visitor commits.
/// Carries hub identity so the form can render "Joining {hub_name}".
#[derive(Debug, Serialize)]
pub struct InviteLinkPreview {
    pub hub_name: String,
    pub hub_slug: String,
    pub expires_at: DateTime<Utc>,
    pub max_uses: Option<i32>,
    pub uses_count: i32,
    pub group_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RedeemInviteLinkRequest {
    pub username: String,
    pub password: String,
    pub display_name: String,
    pub email: String,
}
