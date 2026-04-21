use std::sync::Arc;

use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::auth::{
    create_account_token, generate_verification_token, hash_password, hash_token, verify_password,
};
use crate::config::Config;
use crate::mailer::outbox;
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/auth/signup", post(signup))
        .route("/api/auth/login", post(login))
        .route("/api/auth/verify-email", post(verify_email))
        .route("/api/auth/resend-verification", post(resend_verification))
}

#[derive(Deserialize)]
struct SignupRequest {
    email: String,
    password: String,
}

#[derive(Serialize)]
struct AuthResponse {
    access_token: String,
    account_id: Uuid,
    email: String,
    email_verified: bool,
}

async fn signup(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SignupRequest>,
) -> Result<Json<AuthResponse>, (StatusCode, String)> {
    // TODO: validate email format, password strength, rate-limit by IP
    let hash = hash_password(&req.password)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let account_id: Uuid = sqlx::query_scalar(
        "INSERT INTO accounts (email, password_hash) VALUES ($1, $2) RETURNING id",
    )
    .bind(&req.email)
    .bind(&hash)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        if let Some(db_err) = e.as_database_error() {
            if db_err.is_unique_violation() {
                return (StatusCode::CONFLICT, "account already exists".into());
            }
        }
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    issue_verification_email(&mut tx, account_id, &req.email, &state.config)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let token = create_account_token(
        account_id,
        &req.email,
        state.config.account_token_ttl_secs,
        &state.config.jwt_secret,
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(AuthResponse {
        access_token: token,
        account_id,
        email: req.email,
        email_verified: false,
    }))
}

#[derive(Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
}

async fn login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<AuthResponse>, (StatusCode, String)> {
    let row: Option<(Uuid, String, bool)> =
        sqlx::query_as("SELECT id, password_hash, email_verified FROM accounts WHERE email = $1")
            .bind(&req.email)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (account_id, hash, email_verified) =
        row.ok_or((StatusCode::UNAUTHORIZED, "invalid credentials".into()))?;

    let ok = verify_password(&req.password, &hash)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !ok {
        return Err((StatusCode::UNAUTHORIZED, "invalid credentials".into()));
    }

    let token = create_account_token(
        account_id,
        &req.email,
        state.config.account_token_ttl_secs,
        &state.config.jwt_secret,
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(AuthResponse {
        access_token: token,
        account_id,
        email: req.email,
        email_verified,
    }))
}

#[derive(Deserialize)]
struct VerifyEmailRequest {
    token: String,
}

async fn verify_email(
    State(state): State<Arc<AppState>>,
    Json(req): Json<VerifyEmailRequest>,
) -> Result<Json<AuthResponse>, (StatusCode, String)> {
    let token_hash = hash_token(&req.token);

    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Atomic claim: mark token consumed and flip account.email_verified in
    // one CTE. If the token is missing, expired, or already used, the UPDATE
    // affects zero rows and we get None back.
    let row: Option<(Uuid, String, bool)> = sqlx::query_as(
        r#"
        WITH consumed AS (
            UPDATE email_verification_tokens
            SET consumed_at = now()
            WHERE token_hash = $1
              AND consumed_at IS NULL
              AND expires_at > now()
            RETURNING account_id
        ),
        updated AS (
            UPDATE accounts a
            SET email_verified = true, updated_at = now()
            FROM consumed c
            WHERE a.id = c.account_id
            RETURNING a.id, a.email, a.email_verified
        )
        SELECT id, email, email_verified FROM updated
        "#,
    )
    .bind(&token_hash)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (account_id, email, email_verified) = row.ok_or((
        StatusCode::BAD_REQUEST,
        "invalid or expired verification token".into(),
    ))?;

    // Enqueue welcome mail in the same transaction — after verification, not
    // at signup. Way fewer "welcome to a product I never confirmed I wanted"
    // emails end up in people's inboxes this way.
    let dashboard_url = format!("{}/dashboard", state.config.public_base_url);
    outbox::enqueue_tx(
        &mut tx,
        "account_welcome",
        &email,
        serde_json::json!({
            "account_id": account_id,
            "dashboard_url": dashboard_url,
        }),
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let access_token = create_account_token(
        account_id,
        &email,
        state.config.account_token_ttl_secs,
        &state.config.jwt_secret,
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(AuthResponse {
        access_token,
        account_id,
        email,
        email_verified,
    }))
}

#[derive(Deserialize)]
struct ResendRequest {
    email: String,
}

async fn resend_verification(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ResendRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    // Response is 202 regardless of whether the account exists — otherwise
    // this endpoint becomes a user-enumeration oracle.
    let row: Option<(Uuid, bool)> =
        sqlx::query_as("SELECT id, email_verified FROM accounts WHERE email = $1")
            .bind(&req.email)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some((account_id, false)) = row {
        let mut tx = state
            .pool
            .begin()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        issue_verification_email(&mut tx, account_id, &req.email, &state.config)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    Ok(StatusCode::ACCEPTED)
}

/// Shared: generate a token, store its hash, enqueue the verification email.
/// Must be called inside a caller-owned transaction so the token row and the
/// outbox row land together with the business insert.
async fn issue_verification_email(
    tx: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    email: &str,
    config: &Config,
) -> anyhow::Result<()> {
    let token = generate_verification_token();
    let token_hash = hash_token(&token);

    sqlx::query(
        r#"
        INSERT INTO email_verification_tokens (token_hash, account_id, expires_at)
        VALUES ($1, $2, now() + interval '24 hours')
        "#,
    )
    .bind(&token_hash)
    .bind(account_id)
    .execute(&mut **tx)
    .await?;

    let verify_url = format!("{}/verify-email?token={}", config.public_base_url, token);

    outbox::enqueue_tx(
        tx,
        "account_verify",
        email,
        serde_json::json!({
            "account_id": account_id,
            "verify_url": verify_url,
        }),
    )
    .await?;

    Ok(())
}
