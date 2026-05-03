//! SSO callback — the hub-pod side of the general → hub handoff.
//!
//! Frontend on `slug.matehub.io/auth/sso` receives a `?code=…` URL param from
//! general's redirect, POSTs it here. We:
//!
//!   1. Read our own `HUB_SECRET` (the `JWT_SECRET` env var — they're the
//!      same value in SaaS mode; general provisioned it).
//!   2. Call general's `POST /internal/auth/exchange` using that secret as
//!      Bearer. General identifies us by secret lookup, consumes the code,
//!      returns the account info.
//!   3. Upsert a local `users` row keyed on `account_id` so the account-id
//!      → users.id mapping is stable across re-entries.
//!   4. Ensure hub membership + default group assignment.
//!   5. Issue a hub-scoped JWT (`matehub_common::auth::create_token`, signed
//!      with our `HUB_SECRET`) and return the standard LoginResponse shape
//!      that the `/auth/login` path already returns — so the frontend auth
//!      flow stays unified.

use std::time::Duration;

use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use matehub_common::snowflake;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::auth::{Claims, create_token};

const ACCESS_TOKEN_TTL_SECS: i64 = 30 * 60;

#[derive(Clone)]
pub struct SsoState {
    pub pool: PgPool,
    /// Base URL of the general service (e.g. `https://matehub.io`). In SaaS
    /// this is set via `GENERAL_URL` env. In on-prem — unset; SSO disabled.
    pub general_url: Option<String>,
    /// Our own hub_id, matches `hubs.id` in our local DB. For on-prem and
    /// SaaS: fixed at boot via `HUB_ID` env (or seed's DEV_HUB_ID in dev).
    pub hub_id: i64,
}

pub fn routes(state: SsoState) -> Router {
    Router::new()
        .route("/v1/auth/sso", post(sso_login))
        .with_state(state)
}

#[derive(Deserialize)]
struct SsoRequest {
    code: String,
}

#[derive(Serialize)]
struct LoginResponse {
    access_token: String,
    expires_in: i64,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    user_id: i64,
    username: String,
    display_name: String,
    avatar_url: Option<String>,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    hub_id: i64,
}

/// Shape of `/internal/auth/exchange` response on the general side.
#[derive(Deserialize)]
struct ExchangeResponse {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    account_id: i64,
    email: String,
    display_name: Option<String>,
}

async fn sso_login(
    State(state): State<SsoState>,
    Json(req): Json<SsoRequest>,
) -> Result<Json<LoginResponse>, (StatusCode, String)> {
    let general_url = state.general_url.as_deref().ok_or((
        StatusCode::NOT_IMPLEMENTED,
        "SSO not configured on this hub (on-prem mode)".into(),
    ))?;

    // Use the same secret we sign JWTs with as Bearer to general. It's the
    // shared pre-install between the two services.
    let hub_secret = std::env::var("JWT_SECRET").map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "JWT_SECRET not set — cannot call general".into(),
        )
    })?;

    // Redeem the code with general.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let resp = client
        .post(format!("{general_url}/internal/auth/exchange"))
        .bearer_auth(&hub_secret)
        .json(&serde_json::json!({ "code": req.code }))
        .send()
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "exchange request failed");
            (StatusCode::BAD_GATEWAY, "general unreachable".into())
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        tracing::warn!(%status, %body, "exchange rejected");
        // Propagate upstream status — BAD_REQUEST for bad code, UNAUTHORIZED
        // if general didn't recognize our hub_secret (shouldn't happen in a
        // working install, but surface it cleanly if it does).
        return Err((
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            format!("exchange failed: {body}"),
        ));
    }

    let exchange: ExchangeResponse = resp
        .json()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;

    // Upsert user by account_id. New users get a freshly-generated Snowflake;
    // existing ones keep their users.id. email/display_name refresh on every
    // SSO so general remains source of truth for those two fields.
    let candidate_username = derive_username(&exchange.email);
    let candidate_display_name = exchange
        .display_name
        .clone()
        .unwrap_or_else(|| email_prefix(&exchange.email).to_string());
    let new_user_id = snowflake::next_id();

    #[derive(sqlx::FromRow)]
    struct UserRow {
        id: i64,
        username: String,
        display_name: String,
        avatar_url: Option<String>,
    }

    let user = sqlx::query_as::<_, UserRow>(
        r#"
        INSERT INTO users (id, username, display_name, email, account_id)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (account_id) DO UPDATE
            SET email = EXCLUDED.email
        RETURNING id, username, display_name, avatar_url
        "#,
    )
    .bind(new_user_id)
    .bind(&candidate_username)
    .bind(&candidate_display_name)
    .bind(&exchange.email)
    .bind(exchange.account_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Hub membership. General already validated the account is a member of
    // this hub at /api/hubs/{slug}/enter (it checks general.hub_members), so
    // we mirror that as a local hub_members row on first entry.
    let membership_result = sqlx::query(
        r#"
        INSERT INTO hub_members (hub_id, user_id, role)
        VALUES ($1, $2, 'member')
        ON CONFLICT (hub_id, user_id) DO NOTHING
        "#,
    )
    .bind(state.hub_id)
    .bind(user.id)
    .execute(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Only fire member_joined when we actually inserted a new row -- repeated
    // SSO logins for an existing member shouldn't notify everyone every time.
    if membership_result.rows_affected() > 0 {
        crate::member_events::member_joined(state.hub_id, user.id);
    }

    // Assign to the hub's default group (typically "everyone") so the new
    // user gets baseline READ/WRITE permissions straight away.
    sqlx::query(
        r#"
        INSERT INTO member_groups (hub_id, user_id, group_id)
        SELECT $1, $2, g.id
        FROM groups g
        WHERE g.hub_id = $1 AND g.is_default = true
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(state.hub_id)
    .bind(user.id)
    .execute(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let groups: Vec<i64> =
        sqlx::query_scalar("SELECT group_id FROM member_groups WHERE hub_id = $1 AND user_id = $2")
            .bind(state.hub_id)
            .bind(user.id)
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default();

    // Issue hub-scoped JWT. Signed with HUB_SECRET (= JWT_SECRET env).
    let now = chrono::Utc::now().timestamp();
    let claims = Claims {
        sub: user.id,
        username: user.username.clone(),
        user_type: "permanent".into(),
        hub_id: state.hub_id,
        groups,
        iat: now,
        exp: now + ACCESS_TOKEN_TTL_SECS,
    };
    let access_token = create_token(&claims).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("token issue failed: {e}"),
        )
    })?;

    tracing::info!(
        account_id = exchange.account_id,
        user_id = user.id,
        "SSO login ok"
    );

    Ok(Json(LoginResponse {
        access_token,
        expires_in: ACCESS_TOKEN_TTL_SECS,
        user_id: user.id,
        username: user.username,
        display_name: user.display_name,
        avatar_url: user.avatar_url,
        hub_id: state.hub_id,
    }))
}

/// Username from email. Collision is possible when two different accounts
/// share a local-part (alice@foo.com + alice@bar.com); we defer handling
/// until it actually happens — ON CONFLICT (account_id) DO UPDATE will make
/// the second signup bail with a unique_violation on username, which the
/// frontend can surface as "pick a different name". For MVP good enough.
fn derive_username(email: &str) -> String {
    email.to_string()
}

fn email_prefix(email: &str) -> &str {
    email.split('@').next().unwrap_or(email)
}
