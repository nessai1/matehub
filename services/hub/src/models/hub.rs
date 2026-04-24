use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Hub {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub plan: String,
    #[serde(with = "matehub_common::serde_i64::option_as_string", default)]
    pub creator_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
