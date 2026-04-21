use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Channel {
    pub id: i64,
    pub hub_id: i64,
    pub name: String,
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub channel_type: String,
    pub position: i32,
    pub icon_id: Option<String>,
    pub icon_color: Option<String>,
    pub icon_image_url: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateChannel {
    pub name: String,
    #[serde(rename = "type")]
    pub channel_type: String,
    pub icon_id: Option<String>,
    pub icon_color: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateChannel {
    pub name: Option<String>,
    pub position: Option<i32>,
    pub icon_id: Option<String>,
    pub icon_color: Option<String>,
    pub icon_image_url: Option<String>,
}
