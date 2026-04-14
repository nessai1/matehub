use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{Claims, create_token};

pub fn routes(pool: PgPool) -> Router {
    Router::new()
        .route("/auth/login", post(login))
        .with_state(pool)
}

#[derive(Deserialize)]
struct LoginRequest {
    /// Username or email
    login: String,
    password: String,
    hub_id: Uuid,
}

#[derive(serde::Serialize)]
struct LoginResponse {
    token: String,
    user_id: Uuid,
    username: String,
    display_name: String,
    avatar_url: Option<String>,
    hub_id: Uuid,
    hub_slug: String,
}

/// POST /v1/auth/login -- username + password -> JWT
async fn login(
    State(pool): State<PgPool>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    #[derive(sqlx::FromRow)]
    struct UserRow {
        id: Uuid,
        username: String,
        display_name: String,
        avatar_url: Option<String>,
        password_hash: Option<String>,
    }

    let user = sqlx::query_as::<_, UserRow>(
        "SELECT id, username, display_name, avatar_url, password_hash FROM users
         WHERE username = $1 OR email = $1",
    )
    .bind(&body.login)
    .fetch_optional(&pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::UNAUTHORIZED)?;

    // Verify password
    let hash = user.password_hash.as_deref().ok_or(StatusCode::UNAUTHORIZED)?;
    let valid = bcrypt::verify(&body.password, hash).unwrap_or(false);
    if !valid {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Verify user is member of this hub
    let is_member: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM hub_members WHERE hub_id = $1 AND user_id = $2)",
    )
    .bind(body.hub_id)
    .bind(user.id)
    .fetch_one(&pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if !is_member {
        return Err(StatusCode::FORBIDDEN);
    }

    // Get groups
    let groups: Vec<Uuid> = sqlx::query_scalar(
        "SELECT group_id FROM member_groups WHERE hub_id = $1 AND user_id = $2",
    )
    .bind(body.hub_id)
    .bind(user.id)
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

    // Get hub slug
    let hub_slug: String = sqlx::query_scalar("SELECT slug FROM hubs WHERE id = $1")
        .bind(body.hub_id)
        .fetch_one(&pool)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let now = chrono::Utc::now().timestamp();
    let claims = Claims {
        sub: user.id,
        username: user.username.clone(),
        user_type: "permanent".into(),
        hub_id: body.hub_id,
        groups,
        iat: now,
        exp: now + 86400,
    };

    let token = create_token(&claims).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    tracing::info!(username = %user.username, %body.hub_id, "user logged in");

    Ok(Json(LoginResponse {
        token,
        user_id: user.id,
        username: user.username,
        display_name: user.display_name,
        avatar_url: user.avatar_url,
        hub_id: body.hub_id,
        hub_slug,
    }))
}
