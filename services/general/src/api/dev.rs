use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    routing::get,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::seed;
use crate::models::User;

pub fn routes(pool: PgPool) -> Router {
    Router::new()
        .route("/me", get(dev_me))
        .route("/users", get(dev_users))
        .with_state(pool)
}

#[derive(Deserialize)]
struct DevMeQuery {
    #[serde(default = "default_username")]
    user: String,
}

fn default_username() -> String {
    "alice".to_string()
}

#[derive(Serialize, sqlx::FromRow)]
struct DevMeRow {
    user_id: Uuid,
    username: String,
    display_name: String,
    role: String,
}

#[derive(Serialize)]
struct DevMeResponse {
    user_id: Uuid,
    username: String,
    display_name: String,
    hub_id: Uuid,
    role: String,
    token: String,
}

/// GET /dev/me?user=alice
/// Returns user info + a fake dev token. No real auth.
async fn dev_me(
    State(pool): State<PgPool>,
    Query(query): Query<DevMeQuery>,
) -> Result<Json<DevMeResponse>, StatusCode> {
    let row = sqlx::query_as::<_, DevMeRow>(
        "SELECT u.id AS user_id, u.username, u.display_name, hm.role
         FROM users u
         JOIN hub_members hm ON hm.user_id = u.id
         WHERE u.username = $1 AND hm.hub_id = $2",
    )
    .bind(&query.user)
    .bind(seed::DEV_HUB_ID)
    .fetch_optional(&pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(DevMeResponse {
        user_id: row.user_id,
        username: row.username.clone(),
        display_name: row.display_name,
        hub_id: seed::DEV_HUB_ID,
        role: row.role,
        token: format!("dev-{}-token", row.username),
    }))
}

/// GET /dev/users -- list all dev users
async fn dev_users(
    State(pool): State<PgPool>,
) -> Result<Json<Vec<User>>, StatusCode> {
    let users = sqlx::query_as::<_, User>("SELECT * FROM users ORDER BY username")
        .fetch_all(&pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(users))
}
