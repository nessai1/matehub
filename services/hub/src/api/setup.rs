//! First-run setup endpoints.
//!
//! Boxed deploys ship with an empty database. The very first browser
//! that hits the hub gets a guided onboarding: the visitor picks the
//! admin login + password + display name, then a hub name. This file
//! is the backend half of that wizard.
//!
//! Once `hub_members` is non-empty the endpoints here all return 409
//! Conflict — the wizard is single-shot, like Bitrix's first-run page.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use matehub_common::{perms::bits, snowflake};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::api::auth_api::{ACCESS_TOKEN_TTL_SECS, issue_access_token, issue_refresh_token};

/// Arbitrary u64 used with pg_advisory_xact_lock so two simultaneous
/// setup requests serialize. Without this they'd both pass the
/// "users empty?" check, both INSERT, and the second one would fall
/// over on the slug='default' UNIQUE — but with a 500, not a 409.
const SETUP_LOCK_KEY: i64 = 0x5e_70_a1_77; // "setupAdm"

pub fn routes(pool: PgPool) -> Router {
    Router::new()
        .route("/v1/setup/status", get(status))
        .route("/v1/setup/admin", post(setup_admin))
        .with_state(pool)
}

// ── Status ──────────────────────────────────────────────────────

#[derive(Serialize)]
struct StatusResponse {
    needs_setup: bool,
    // When setup is done, surface the hub's identity so unauthenticated UI
    // (the login page) doesn't have to guess. Snowflake doesn't fit in a JS
    // number, so `hub_id` ships as a string -- same wire shape every other
    // i64 takes via `serde_i64::as_string` elsewhere. Absent when the
    // wizard hasn't run yet.
    //
    // Why this lives on /setup/status instead of a new /hubs/current:
    // the wizard endpoint is the only one a fresh-deploy SPA can call
    // without an auth token, and a one-shot fetch covering both "is the
    // app set up?" and "which hub am I?" keeps the unauthenticated
    // surface area minimal.
    #[serde(skip_serializing_if = "Option::is_none")]
    hub_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hub_slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hub_name: Option<String>,
}

async fn status(State(pool): State<PgPool>) -> Result<Json<StatusResponse>, StatusCode> {
    let any_member: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM hub_members LIMIT 1)")
        .fetch_one(&pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if !any_member {
        return Ok(Json(StatusResponse {
            needs_setup: true,
            hub_id: None,
            hub_slug: None,
            hub_name: None,
        }));
    }

    // Box deploys have exactly one hub. SaaS multi-tenant uses a subdomain
    // resolver instead and never hits this endpoint with the "give me the
    // hub" expectation. `LIMIT 1` keeps the response shape predictable if
    // the multi-tenant case ever leaks here -- the SPA gets *a* hub, not a
    // crash.
    type HubRow = (i64, String, String);
    let hub: Option<HubRow> = sqlx::query_as("SELECT id, slug, name FROM hubs ORDER BY id LIMIT 1")
        .fetch_optional(&pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let (hub_id, hub_slug, hub_name) = match hub {
        Some((id, slug, name)) => (Some(id.to_string()), Some(slug), Some(name)),
        None => (None, None, None),
    };

    Ok(Json(StatusResponse {
        needs_setup: false,
        hub_id,
        hub_slug,
        hub_name,
    }))
}

// ── Admin setup ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct SetupAdminRequest {
    username: String,
    password: String,
    display_name: String,
    email: Option<String>,
    hub_name: String,
}

#[derive(Serialize)]
struct SetupAdminResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    user_id: i64,
    username: String,
    display_name: String,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    hub_id: i64,
    hub_slug: String,
    hub_name: String,
}

async fn setup_admin(
    State(pool): State<PgPool>,
    Json(body): Json<SetupAdminRequest>,
) -> Result<Json<SetupAdminResponse>, StatusCode> {
    if body.username.is_empty() || body.password.is_empty() || body.hub_name.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut tx = pool
        .begin()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Serialize concurrent first-run attempts so only one wins.
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(SETUP_LOCK_KEY)
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let already_set_up: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM hub_members LIMIT 1)")
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if already_set_up {
        return Err(StatusCode::CONFLICT);
    }

    // ── 1. Admin user ────────────────────────────────────────
    let password_hash = bcrypt::hash(&body.password, bcrypt::DEFAULT_COST)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let user_id = snowflake::next_id();

    sqlx::query(
        "INSERT INTO users (id, username, display_name, email, password_hash)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(&body.username)
    .bind(&body.display_name)
    .bind(&body.email)
    .bind(&password_hash)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        tracing::warn!(err = %e, "setup: insert user failed");
        // unique_violation on username → 409
        match e.as_database_error().and_then(|e| e.code()) {
            Some(code) if code == "23505" => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    })?;

    // ── 2. Hub ────────────────────────────────────────────────
    let hub_id = snowflake::next_id();
    sqlx::query(
        "INSERT INTO hubs (id, name, slug, plan, creator_id)
         VALUES ($1, $2, 'default', 'free', $3)",
    )
    .bind(hub_id)
    .bind(&body.hub_name)
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // RLS gate for groups/members/channels inserts inside this txn.
    crate::db::rls::set_hub_context_local(&mut *tx, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // ── 3. Groups: admin (ALL) + everyone (default, no perms) ─
    let admin_group_id = snowflake::next_id();
    let everyone_group_id = snowflake::next_id();

    sqlx::query(
        "INSERT INTO groups (id, hub_id, name, color, position, is_default, hub_permissions)
         VALUES ($1, $2, 'admin',    '#E74C3C', 0,   false, $3),
                ($4, $2, 'everyone', '#99AAB5', 100, true,  0)",
    )
    .bind(admin_group_id)
    .bind(hub_id)
    .bind(bits::ALL)
    .bind(everyone_group_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // ── 4. Hub membership for admin ───────────────────────────
    sqlx::query("INSERT INTO hub_members (hub_id, user_id, role) VALUES ($1, $2, 'admin')")
        .bind(hub_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    sqlx::query(
        "INSERT INTO member_groups (hub_id, user_id, group_id) VALUES ($1, $2, $3), ($1, $2, $4)",
    )
    .bind(hub_id)
    .bind(user_id)
    .bind(admin_group_id)
    .bind(everyone_group_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // ── 5. #general text channel ──────────────────────────────
    //
    // The runtime path (api::channels::create_channel) writes both the
    // channel row AND a channel_permissions row granting MEMBER_CHANNEL
    // bits to the everyone group; without that everyone row, non-admin
    // members hit `chat::access::check_uncached` with no matching row in
    // `channel_permissions JOIN member_groups` and get 0 effective bits,
    // i.e. a 403 on /messages. Admin sidesteps it via `is_admin = true`.
    //
    // This first-run wizard used to skip the permissions step (the seed
    // path had it, the runtime path was fixed separately, this one was
    // missed). On a box install the symptom was: alice signs up as admin
    // → invites Bob through the new invite-link flow → Bob redeems,
    // lands in `everyone` + `guests`, but every text channel returns 403
    // because no group has any allow_bits on it. Both INSERTs live in
    // the same tx so a #general write that can't be granted MEMBER bits
    // rolls back the whole hub bootstrap instead of leaving a broken
    // half-state.
    let general_channel_id = snowflake::next_id();
    sqlx::query(
        "INSERT INTO channels (id, hub_id, name, type, position) VALUES ($1, $2, 'general', 'text', 0)",
    )
    .bind(general_channel_id)
    .bind(hub_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    sqlx::query(
        "INSERT INTO channel_permissions (channel_id, group_id, allow_bits, deny_bits)
         VALUES ($1, $2, $3, 0)",
    )
    .bind(general_channel_id)
    .bind(everyone_group_id)
    .bind(bits::MEMBER_CHANNEL)
    .execute(&mut *tx)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    tx.commit()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // ── 6. Issue tokens — admin is logged in immediately ─────
    let groups = vec![admin_group_id, everyone_group_id];
    let access_token = issue_access_token(user_id, &body.username, hub_id, &groups)?;
    let refresh_token = issue_refresh_token(&pool, user_id, hub_id).await?;

    tracing::info!(
        username = %body.username,
        %hub_id,
        hub_name = %body.hub_name,
        "boxed hub initial setup completed"
    );

    Ok(Json(SetupAdminResponse {
        access_token,
        refresh_token,
        expires_in: ACCESS_TOKEN_TTL_SECS,
        user_id,
        username: body.username,
        display_name: body.display_name,
        hub_id,
        hub_slug: "default".into(),
        hub_name: body.hub_name,
    }))
}
