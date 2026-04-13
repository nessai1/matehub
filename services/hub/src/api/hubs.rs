use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{Channel, Hub};

pub fn routes(pool: PgPool) -> Router {
    Router::new()
        .route("/hubs/{hub_id}", get(get_hub))
        .route("/hubs/{hub_id}/channels", get(get_channels))
        .route("/hubs/{hub_id}/members", get(get_members))
        .with_state(pool)
}

async fn get_hub(
    State(pool): State<PgPool>,
    Path(hub_id): Path<Uuid>,
) -> Result<Json<Hub>, StatusCode> {
    let hub = sqlx::query_as::<_, Hub>("SELECT * FROM hubs WHERE id = $1")
        .bind(hub_id)
        .fetch_optional(&pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(hub))
}

async fn get_channels(
    State(pool): State<PgPool>,
    Path(hub_id): Path<Uuid>,
) -> Result<Json<Vec<Channel>>, StatusCode> {
    let channels =
        sqlx::query_as::<_, Channel>("SELECT * FROM channels WHERE hub_id = $1 ORDER BY position")
            .bind(hub_id)
            .fetch_all(&pool)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(channels))
}

#[derive(Serialize, sqlx::FromRow)]
struct MemberRow {
    user_id: Uuid,
    username: String,
    display_name: String,
    avatar_url: Option<String>,
    role: String,
}

async fn get_members(
    State(pool): State<PgPool>,
    Path(hub_id): Path<Uuid>,
) -> Result<Json<Vec<MemberRow>>, StatusCode> {
    let members = sqlx::query_as::<_, MemberRow>(
        "SELECT u.id AS user_id, u.username, u.display_name, u.avatar_url, hm.role
         FROM users u
         JOIN hub_members hm ON hm.user_id = u.id
         WHERE hm.hub_id = $1
         ORDER BY hm.joined_at",
    )
    .bind(hub_id)
    .fetch_all(&pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(members))
}
