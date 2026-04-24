//! SSO handoff: matehub.io (general) → slug.matehub.io (hub pod).
//!
//! Two endpoints live here:
//!
//!   1. `POST /api/hubs/{slug}/enter` — authed by the account JWT. User
//!      clicked "Enter Hub X" on the dashboard. We mint a single-use
//!      `auth_code` (30s TTL, rows in `auth_codes`) and return the URL the
//!      frontend should redirect the browser to:
//!      `https://{slug}.matehub.io/auth/sso?code={code}`.
//!
//!   2. `POST /internal/auth/exchange` — Bearer-authed by a hub pod's own
//!      `HUB_SECRET`. We look the hub up by that secret (unique index),
//!      atomically consume the code if it matches this hub, and return the
//!      account info the hub needs to provision its local `users` row.
//!
//! The pre-shared secret is established at hub creation: general generates
//! a 32-byte random value, stores it in `hubs.hub_secret`, and hands it to
//! the hub pod via K8s Secret → `JWT_SECRET` env var. Rotating is a matter
//! of updating the row and rolling the pod; out of MVP scope.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use rand::{Rng, rng};
use serde::{Deserialize, Serialize};

use crate::api::extract::Authed;
use crate::state::AppState;

const CODE_TTL_SECS: i64 = 30;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/hubs/{slug}/enter", post(enter_hub))
        .route("/internal/auth/exchange", post(exchange_code))
}

// ── 1. /api/hubs/{slug}/enter ────────────────────

#[derive(Serialize)]
struct EnterHubResponse {
    /// URL the frontend navigates the browser to. The hub pod's /auth/sso
    /// handler runs when the browser lands there.
    redirect_url: String,
}

async fn enter_hub(
    State(state): State<Arc<AppState>>,
    Authed(claims): Authed,
    Path(slug): Path<String>,
) -> Result<Json<EnterHubResponse>, (StatusCode, String)> {
    // Find the hub + verify this account is a member of it. Non-members get
    // a 404 rather than a 403 — we don't confirm the hub's existence to
    // random accounts that happen to know the slug.
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT h.id FROM hubs h
        JOIN hub_members hm ON hm.hub_id = h.id
        WHERE h.slug = $1 AND hm.account_id = $2 AND h.status = 'ready'
        "#,
    )
    .bind(&slug)
    .bind(claims.sub)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (hub_id,) = row.ok_or((StatusCode::NOT_FOUND, "hub not found".into()))?;

    let code = generate_code();
    sqlx::query(
        r#"
        INSERT INTO auth_codes (code, account_id, hub_id, expires_at)
        VALUES ($1, $2, $3, now() + make_interval(secs => $4))
        "#,
    )
    .bind(&code)
    .bind(claims.sub)
    .bind(hub_id)
    .bind(CODE_TTL_SECS as f64)
    .execute(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // URL template from config. In dev — a single-port path prefix
    // (http://localhost:3002/hub/{slug}/auth/sso). In prod —
    // https://{slug}.matehub.io/auth/sso.
    let redirect_url = state
        .config
        .hub_sso_url(&slug, &code);

    tracing::info!(%slug, account_id = claims.sub, "SSO auth_code minted");

    Ok(Json(EnterHubResponse { redirect_url }))
}

// ── 2. /internal/auth/exchange ───────────────────

#[derive(Deserialize)]
struct ExchangeRequest {
    code: String,
}

#[derive(Serialize)]
struct ExchangeResponse {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    account_id: i64,
    email: String,
    display_name: Option<String>,
}

async fn exchange_code(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ExchangeRequest>,
) -> Result<Json<ExchangeResponse>, (StatusCode, String)> {
    let hub_secret = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or((StatusCode::UNAUTHORIZED, "missing Bearer".into()))?;

    // Identify the hub by its pre-shared secret. The UNIQUE index on
    // hubs.hub_secret guarantees at most one row.
    let hub_row: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM hubs WHERE hub_secret = $1")
            .bind(hub_secret)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (hub_id,) = hub_row.ok_or((StatusCode::UNAUTHORIZED, "unknown hub secret".into()))?;

    // Atomic claim: mark the code consumed and return the account tied to it,
    // but ONLY if the code was for this specific hub. A hub holding a leaked
    // code for another hub still won't get anything.
    let claimed: Option<(i64,)> = sqlx::query_as(
        r#"
        UPDATE auth_codes
        SET consumed_at = now()
        WHERE code = $1
          AND hub_id = $2
          AND consumed_at IS NULL
          AND expires_at > now()
        RETURNING account_id
        "#,
    )
    .bind(&req.code)
    .bind(hub_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (account_id,) = claimed.ok_or((
        StatusCode::BAD_REQUEST,
        "invalid, expired, or already-consumed code".into(),
    ))?;

    let acc: (String, Option<String>) =
        sqlx::query_as("SELECT email, display_name FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_one(&state.pool)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tracing::info!(%hub_id, account_id, "SSO code consumed");

    Ok(Json(ExchangeResponse {
        account_id,
        email: acc.0,
        display_name: acc.1,
    }))
}

// ── Code generation ──────────────────────────────

/// 32 bytes of randomness, URL-safe base64 without padding. 256 bits of
/// entropy — can't practically brute-force in a 30-second window.
fn generate_code() -> String {
    let bytes: [u8; 32] = rng().random();
    base64_url_encode(&bytes)
}

fn base64_url_encode(data: &[u8]) -> String {
    const CHARS: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len() * 4 / 3 + 1);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        out.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(CHARS[(triple & 0x3F) as usize] as char);
        }
    }
    out
}
