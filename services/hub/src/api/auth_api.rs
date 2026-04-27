use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use matehub_common::snowflake;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{Claims, create_token};

pub(super) const ACCESS_TOKEN_TTL_SECS: i64 = 30 * 60; // 30 minutes
const REFRESH_TOKEN_TTL_SECS: i64 = 30 * 24 * 3600; // 30 days

pub fn routes(pool: PgPool) -> Router {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout))
        .with_state(pool)
}

// ── Types ───────────────────────────────────────

#[derive(Deserialize)]
struct LoginRequest {
    login: String,
    password: String,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    hub_id: i64,
    #[serde(default)]
    remember_me: bool,
}

#[derive(serde::Serialize)]
struct LoginResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    user_id: i64,
    username: String,
    display_name: String,
    avatar_url: Option<String>,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    hub_id: i64,
    hub_slug: String,
}

#[derive(Deserialize)]
struct RefreshRequest {
    refresh_token: String,
}

#[derive(serde::Serialize)]
struct RefreshResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
}

#[derive(Deserialize)]
struct LogoutRequest {
    refresh_token: String,
}

// ── Login ───────────────────────────────────────

async fn login(
    State(pool): State<PgPool>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    #[derive(sqlx::FromRow)]
    struct UserRow {
        id: i64,
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

    let hash = user.password_hash.as_deref().ok_or(StatusCode::UNAUTHORIZED)?;
    if !bcrypt::verify(&body.password, hash).unwrap_or(false) {
        return Err(StatusCode::UNAUTHORIZED);
    }

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

    let groups: Vec<i64> = sqlx::query_scalar(
        "SELECT group_id FROM member_groups WHERE hub_id = $1 AND user_id = $2",
    )
    .bind(body.hub_id)
    .bind(user.id)
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

    let hub_slug: String = sqlx::query_scalar("SELECT slug FROM hubs WHERE id = $1")
        .bind(body.hub_id)
        .fetch_one(&pool)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let access_token = issue_access_token(user.id, &user.username, body.hub_id, &groups)?;

    let refresh_token = if body.remember_me {
        Some(issue_refresh_token(&pool, user.id, body.hub_id).await?)
    } else {
        None
    };

    tracing::info!(
        username = %user.username,
        hub_id = body.hub_id,
        remember = body.remember_me,
        "user logged in"
    );

    Ok(Json(LoginResponse {
        access_token,
        refresh_token,
        expires_in: ACCESS_TOKEN_TTL_SECS,
        user_id: user.id,
        username: user.username,
        display_name: user.display_name,
        avatar_url: user.avatar_url,
        hub_id: body.hub_id,
        hub_slug,
    }))
}

// ── Refresh ─────────────────────────────────────

async fn refresh(
    State(pool): State<PgPool>,
    Json(body): Json<RefreshRequest>,
) -> Result<Json<RefreshResponse>, StatusCode> {
    let token_hash = hash_token(&body.refresh_token);

    #[derive(sqlx::FromRow)]
    struct RefreshRow {
        user_id: i64,
        hub_id: i64,
        expires_at: chrono::DateTime<chrono::Utc>,
    }

    let row = sqlx::query_as::<_, RefreshRow>(
        "SELECT user_id, hub_id, expires_at FROM refresh_tokens WHERE token_hash = $1",
    )
    .bind(&token_hash)
    .fetch_optional(&pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::UNAUTHORIZED)?;

    if row.expires_at < chrono::Utc::now() {
        let _ = sqlx::query("DELETE FROM refresh_tokens WHERE token_hash = $1")
            .bind(&token_hash)
            .execute(&pool)
            .await;
        return Err(StatusCode::UNAUTHORIZED);
    }

    let username: String = sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
        .bind(row.user_id)
        .fetch_one(&pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let groups: Vec<i64> = sqlx::query_scalar(
        "SELECT group_id FROM member_groups WHERE hub_id = $1 AND user_id = $2",
    )
    .bind(row.hub_id)
    .bind(row.user_id)
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

    let access_token = issue_access_token(row.user_id, &username, row.hub_id, &groups)?;

    let _ = sqlx::query("DELETE FROM refresh_tokens WHERE token_hash = $1")
        .bind(&token_hash)
        .execute(&pool)
        .await;

    let new_refresh = issue_refresh_token(&pool, row.user_id, row.hub_id).await?;

    tracing::debug!(%username, "token refreshed (rotated)");

    Ok(Json(RefreshResponse {
        access_token,
        refresh_token: new_refresh,
        expires_in: ACCESS_TOKEN_TTL_SECS,
    }))
}

// ── Logout ──────────────────────────────────────

async fn logout(
    State(pool): State<PgPool>,
    Json(body): Json<LogoutRequest>,
) -> StatusCode {
    let token_hash = hash_token(&body.refresh_token);
    let _ = sqlx::query("DELETE FROM refresh_tokens WHERE token_hash = $1")
        .bind(&token_hash)
        .execute(&pool)
        .await;
    StatusCode::NO_CONTENT
}

// ── Helpers ─────────────────────────────────────

pub(super) fn issue_access_token(
    user_id: i64,
    username: &str,
    hub_id: i64,
    groups: &[i64],
) -> Result<String, StatusCode> {
    let now = chrono::Utc::now().timestamp();
    let claims = Claims {
        sub: user_id,
        username: username.to_string(),
        user_type: "permanent".into(),
        hub_id,
        groups: groups.to_vec(),
        iat: now,
        exp: now + ACCESS_TOKEN_TTL_SECS,
    };
    create_token(&claims).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub(super) async fn issue_refresh_token(
    pool: &PgPool,
    user_id: i64,
    hub_id: i64,
) -> Result<String, StatusCode> {
    // Random opaque token (two UUIDs concatenated — 256 bits of entropy).
    let raw_token = Uuid::new_v4().to_string() + &Uuid::new_v4().to_string();
    let token_hash = hash_token(&raw_token);
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(REFRESH_TOKEN_TTL_SECS);

    // Keep max 5 active refresh tokens per user+hub; delete the oldest.
    sqlx::query(
        "DELETE FROM refresh_tokens WHERE id IN (
            SELECT id FROM refresh_tokens
            WHERE user_id = $1 AND hub_id = $2
            ORDER BY created_at DESC
            OFFSET 4
        )",
    )
    .bind(user_id)
    .bind(hub_id)
    .execute(pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    sqlx::query(
        "INSERT INTO refresh_tokens (id, user_id, hub_id, token_hash, expires_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(snowflake::next_id())
    .bind(user_id)
    .bind(hub_id)
    .bind(&token_hash)
    .bind(expires_at)
    .execute(pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(raw_token)
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}
