use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{Claims, create_token};

pub fn routes(pool: PgPool) -> Router {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/dev-login", post(dev_login))
        .with_state(pool)
}

// ── Dev login (no password, just username) ──────────

#[derive(Deserialize)]
struct DevLoginRequest {
    username: String,
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

/// POST /v1/auth/dev-login -- dev mode only, no password
async fn dev_login(
    State(pool): State<PgPool>,
    Json(body): Json<DevLoginRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    #[derive(sqlx::FromRow)]
    struct UserRow {
        id: Uuid,
        username: String,
        display_name: String,
        avatar_url: Option<String>,
    }

    let user = sqlx::query_as::<_, UserRow>("SELECT id, username, display_name, avatar_url FROM users WHERE username = $1")
        .bind(&body.username)
        .fetch_optional(&pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    // Get user's groups in this hub
    let groups: Vec<Uuid> = sqlx::query_scalar(
        "SELECT group_id FROM member_groups WHERE hub_id = $1 AND user_id = $2",
    )
    .bind(body.hub_id)
    .bind(user.id)
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

    // Get hub slug
    let hub_slug: String =
        sqlx::query_scalar("SELECT slug FROM hubs WHERE id = $1")
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
        exp: now + 86400, // 24h
    };

    let token = create_token(&claims).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

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

// ── Real login (email + password) -- placeholder ────

#[derive(Deserialize)]
struct LoginRequest {
    #[allow(dead_code)]
    email: String,
    #[allow(dead_code)]
    password: String,
}

/// POST /v1/auth/login -- real auth (TODO: implement password hashing)
async fn login(
    Json(_body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    // TODO: lookup user by email, verify bcrypt hash, issue JWT
    Err(StatusCode::NOT_IMPLEMENTED)
}
