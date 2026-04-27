use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Invitation {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub id: i64,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub hub_id: i64,
    pub token: String,
    pub username: String,
    pub email: Option<String>,
    #[serde(with = "matehub_common::serde_i64::option_as_string", default)]
    pub group_id: Option<i64>,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub created_by: i64,
    pub used_at: Option<DateTime<Utc>>,
    #[serde(with = "matehub_common::serde_i64::option_as_string", default)]
    pub used_by: Option<i64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateInvitationRequest {
    pub username: String,
    pub email: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InvitationLink {
    pub token: String,
    pub username: String,
    pub email: Option<String>,
    pub invite_url: String,
}

/// Returned by the public `GET /v1/invitations/{token}` preview endpoint.
/// Carries hub identity so the accept page can show "Joining {hub_name}".
#[derive(Debug, Serialize)]
pub struct InvitationPreview {
    pub hub_name: String,
    pub username: String,
    pub email: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AcceptInvitationRequest {
    pub display_name: String,
    pub password: String,
}
