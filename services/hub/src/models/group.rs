use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Group {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub id: i64,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    pub hub_id: i64,
    pub name: String,
    pub color: Option<String>,
    pub position: i32,
    pub is_default: bool,
    /// Hub-level permission bits (bits 7-13).
    pub hub_permissions: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateGroup {
    pub name: String,
    pub color: Option<String>,
    pub hub_permissions: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateGroup {
    pub name: Option<String>,
    pub color: Option<String>,
    pub position: Option<i32>,
    pub hub_permissions: Option<i32>,
}
