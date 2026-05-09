//! General invite links — third invite mechanism.
//!
//! Companion to `temp_users.rs` (TTL guests) and `invitations.rs` (admin-
//! pre-allocated permanent users). One link is shared with many people;
//! each visitor self-registers (login + password + display_name + email)
//! at `/signup/{token}` and lands as a real `users` row.
//!
//! Slot claim is atomic: redeem runs an `UPDATE ... WHERE <alive> RETURNING`,
//! so concurrent racers on the last slot lose without serialising the
//! whole table. The increment lives inside the same tx as the user
//! INSERT, so a username UNIQUE collision rolls back the slot too —
//! collisions never burn a slot.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use matehub_common::{
    perms::{bits, resolve_user_perms},
    snowflake,
};
use serde::Serialize;
use sqlx::PgPool;

use crate::api::auth_api::{ACCESS_TOKEN_TTL_SECS, issue_access_token, issue_refresh_token};
use crate::api::invitations::{generate_token, is_valid_email};
use crate::auth::AuthUser;
use crate::db::rls::hub_connection;
use crate::models::invite_link::{
    CreateInviteLinkRequest, InviteLinkPreview, InviteLinkResponse, RedeemInviteLinkRequest,
};

pub fn routes(pool: PgPool) -> Router {
    Router::new()
        .route("/v1/hubs/{hub_id}/invite-links", post(create_invite_link))
        .route("/v1/invite-links/{token}", get(get_invite_link))
        .route("/v1/invite-links/{token}/redeem", post(redeem_invite_link))
        .with_state(pool)
}

// ── Discriminated error body ────────────────────────────────────
//
// `{"error": "username_taken"}` — the FE field-scoped errors switch on this
// code. Mirrors `invitations.rs` exactly so admin-side and signup-side
// forms can share the same `applyServerError` shape.

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
}

impl ErrorBody {
    fn into_response(self, status: StatusCode) -> Response {
        (status, Json(self)).into_response()
    }
}

fn err(code: &'static str, status: StatusCode) -> Response {
    ErrorBody { error: code }.into_response(status)
}

fn internal() -> Response {
    err("internal", StatusCode::INTERNAL_SERVER_ERROR)
}

// ── Create (admin) ──────────────────────────────────────────────

async fn create_invite_link(
    State(pool): State<PgPool>,
    Path(hub_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<CreateInviteLinkRequest>,
) -> Result<(StatusCode, Json<InviteLinkResponse>), Response> {
    // Perm check first — no point validating a body the caller can't act on.
    let caller = resolve_user_perms(&pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| internal())?;
    if !caller.has(bits::INVITE_PERMANENT) {
        return Err(err("forbidden", StatusCode::FORBIDDEN));
    }

    // Refuse to issue a link that's born dead. Both checks are also covered
    // by the redeem-time predicate, but failing fast at create gives the
    // admin a clean 400 instead of a confusing-later-only 410.
    if body.expires_at <= Utc::now() {
        return Err(err("expires_in_past", StatusCode::BAD_REQUEST));
    }
    if let Some(n) = body.max_uses
        && n <= 0
    {
        return Err(err("invalid_max_uses", StatusCode::BAD_REQUEST));
    }

    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| internal())?;

    // If a target group is requested, verify it lives in this hub. RLS would
    // already filter cross-hub rows, but a missing group_id deserves a
    // structured error rather than silent fallback to everyone-only.
    if let Some(gid) = body.group_id {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM groups WHERE id = $1 AND hub_id = $2)",
        )
        .bind(gid)
        .bind(hub_id)
        .fetch_one(&mut *conn)
        .await
        .map_err(|_| internal())?;
        if !exists {
            return Err(err("invalid_group", StatusCode::BAD_REQUEST));
        }
    }

    let id = snowflake::next_id();
    let token = generate_token();

    sqlx::query(
        "INSERT INTO invite_links
            (id, hub_id, token, expires_at, max_uses, group_id, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(hub_id)
    .bind(&token)
    .bind(body.expires_at)
    .bind(body.max_uses)
    .bind(body.group_id)
    .bind(auth.0.sub)
    .execute(&mut *conn)
    .await
    .map_err(|_| internal())?;

    Ok((
        StatusCode::CREATED,
        Json(InviteLinkResponse {
            id,
            token: token.clone(),
            invite_url: format!("/signup/{token}"),
            expires_at: body.expires_at,
            max_uses: body.max_uses,
            uses_count: 0,
            group_id: body.group_id,
        }),
    ))
}

// ── Public preview ──────────────────────────────────────────────
//
// Read-only, no RLS context. Owner-of-table bypasses RLS in our setup
// (no FORCE ROW LEVEL SECURITY), and the `, true` form on the policy keeps
// it consistent regardless. The 410 predicate here is informational —
// the authoritative gate is the atomic UPDATE in `redeem_invite_link`.

async fn get_invite_link(
    State(pool): State<PgPool>,
    Path(token): Path<String>,
) -> Result<Json<InviteLinkPreview>, StatusCode> {
    type Row = (
        DateTime<Utc>,         // expires_at
        Option<i32>,           // max_uses
        i32,                   // uses_count
        Option<DateTime<Utc>>, // revoked_at
        String,                // hub_name
        String,                // hub_slug
        Option<String>,        // group_name
    );

    let row: Option<Row> = sqlx::query_as(
        "SELECT il.expires_at, il.max_uses, il.uses_count, il.revoked_at,
                h.name, h.slug,
                g.name AS group_name
         FROM invite_links il
         JOIN hubs h ON h.id = il.hub_id
         LEFT JOIN groups g ON g.id = il.group_id
         WHERE il.token = $1",
    )
    .bind(&token)
    .fetch_optional(&pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let (expires_at, max_uses, uses_count, revoked_at, hub_name, hub_slug, group_name) =
        row.ok_or(StatusCode::NOT_FOUND)?;

    if revoked_at.is_some() {
        return Err(StatusCode::GONE);
    }
    if expires_at <= Utc::now() {
        return Err(StatusCode::GONE);
    }
    if let Some(cap) = max_uses
        && uses_count >= cap
    {
        return Err(StatusCode::GONE);
    }

    Ok(Json(InviteLinkPreview {
        hub_name,
        hub_slug,
        expires_at,
        max_uses,
        uses_count,
        group_name,
    }))
}

// ── Redeem (public, registers the visitor) ──────────────────────

#[derive(Serialize)]
struct RedeemResponse {
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
}

async fn redeem_invite_link(
    State(pool): State<PgPool>,
    Path(token): Path<String>,
    Json(body): Json<RedeemInviteLinkRequest>,
) -> Result<Json<RedeemResponse>, Response> {
    // ── Body validation ─────────────────────────────────────────
    let username = body.username.trim();
    let display_name = body.display_name.trim();
    let email = body.email.trim();

    if !is_valid_username(username) {
        return Err(err("username_invalid", StatusCode::BAD_REQUEST));
    }
    if body.password.len() < 6 {
        return Err(err("password_too_short", StatusCode::BAD_REQUEST));
    }
    if display_name.is_empty() || display_name.chars().count() > 64 {
        return Err(err("display_name_invalid", StatusCode::BAD_REQUEST));
    }
    if !is_valid_email(email) {
        return Err(err("email_invalid", StatusCode::BAD_REQUEST));
    }

    // ── Atomic slot claim + user creation in one tx ─────────────
    //
    // Order matters: claim FIRST (still without RLS context — we don't know
    // hub_id yet), then SET LOCAL with the returned hub_id, then do the
    // hub-scoped inserts. Mirrors `invitations.rs` accept_invitation flow.
    let mut tx = pool.begin().await.map_err(|_| internal())?;

    type ClaimRow = (i64, i64, Option<i64>); // id, hub_id, group_id
    let claim: Option<ClaimRow> = sqlx::query_as(
        "UPDATE invite_links
            SET uses_count = uses_count + 1
          WHERE token = $1
            AND revoked_at IS NULL
            AND expires_at > now()
            AND (max_uses IS NULL OR uses_count < max_uses)
         RETURNING id, hub_id, group_id",
    )
    .bind(&token)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| internal())?;

    let Some((invite_link_id, hub_id, link_group_id)) = claim else {
        // Either no such token, or revoked, or expired, or maxed out. We
        // collapse all of these to 410 — telling the client which one only
        // helps an attacker enumerate states.
        return Err(err("link_unavailable", StatusCode::GONE));
    };

    // RLS context for hub_members + member_groups inserts and for the
    // default-group lookup. Owners bypass RLS in our setup, but be
    // explicit so a future tightening (FORCE ROW LEVEL SECURITY) doesn't
    // silently break this path.
    sqlx::query(&format!("SET LOCAL app.current_hub_id = '{hub_id}'"))
        .execute(&mut *tx)
        .await
        .map_err(|_| internal())?;

    // Pre-flight uniqueness checks. The race-loser still hits the unique
    // indexes on users.username / users.email, but pre-flight gives the
    // common case a discriminated 409 instead of a generic
    // "duplicate key value" the FE can't translate to a field-scoped
    // message.
    let username_taken: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE username = $1)")
            .bind(username)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| internal())?;
    if username_taken {
        return Err(err("username_taken", StatusCode::CONFLICT));
    }

    let email_taken: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE email = $1)")
            .bind(email)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| internal())?;
    if email_taken {
        return Err(err("email_taken", StatusCode::CONFLICT));
    }

    let password_hash =
        bcrypt::hash(&body.password, bcrypt::DEFAULT_COST).map_err(|_| internal())?;
    let user_id = snowflake::next_id();

    let user_insert = sqlx::query(
        "INSERT INTO users (id, username, display_name, email, password_hash)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(username)
    .bind(display_name)
    .bind(email)
    .bind(&password_hash)
    .execute(&mut *tx)
    .await;
    if let Err(e) = user_insert {
        // Race-loser: someone else snagged the username/email between the
        // pre-flight check and this INSERT. Map to the same field-scoped
        // codes; we don't know which constraint fired without parsing,
        // so 409 with `username_taken` covers the common case (email is
        // optional in the existing accept flow but mandatory here).
        return Err(match e.as_database_error().and_then(|d| d.code()) {
            Some(code) if code == "23505" => err("username_taken", StatusCode::CONFLICT),
            _ => internal(),
        });
    }

    sqlx::query("INSERT INTO hub_members (hub_id, user_id, role) VALUES ($1, $2, 'member')")
        .bind(hub_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| internal())?;

    let everyone_group_id: i64 =
        sqlx::query_scalar("SELECT id FROM groups WHERE hub_id = $1 AND is_default = true LIMIT 1")
            .bind(hub_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| internal())?;

    let group_ids: Vec<i64> = match link_group_id {
        Some(gid) if gid != everyone_group_id => vec![everyone_group_id, gid],
        _ => vec![everyone_group_id],
    };

    for gid in &group_ids {
        sqlx::query(
            "INSERT INTO member_groups (hub_id, user_id, group_id) VALUES ($1, $2, $3)
             ON CONFLICT DO NOTHING",
        )
        .bind(hub_id)
        .bind(user_id)
        .bind(gid)
        .execute(&mut *tx)
        .await
        .map_err(|_| internal())?;
    }

    tx.commit().await.map_err(|_| internal())?;

    // Notify presence WS — same hook the existing accept flow fires
    // (invitations.rs:351).
    crate::member_events::member_joined(hub_id, user_id);

    let hub_slug: String = sqlx::query_scalar("SELECT slug FROM hubs WHERE id = $1")
        .bind(hub_id)
        .fetch_one(&pool)
        .await
        .map_err(|_| internal())?;

    let access_token =
        issue_access_token(user_id, username, hub_id, &group_ids).map_err(|_| internal())?;
    let refresh_token = issue_refresh_token(&pool, user_id, hub_id)
        .await
        .map_err(|_| internal())?;

    tracing::info!(
        username = %username,
        %hub_id,
        %invite_link_id,
        "invite-link redeemed"
    );

    Ok(Json(RedeemResponse {
        access_token,
        refresh_token,
        expires_in: ACCESS_TOKEN_TTL_SECS,
        user_id,
        username: username.to_string(),
        display_name: display_name.to_string(),
        hub_id,
        hub_slug,
    }))
}

// ── Username validation ─────────────────────────────────────────
//
// The signup form lets the visitor pick any username they like. Existing
// flows take the username from an admin (permanent invite) or autogenerate
// it (temp guest), so this is the first place we have to defend the format
// against arbitrary user input.
//
// 3-32 chars, alphanumerics plus `.`, `_`, `-`. Conservative on purpose:
// rejecting unicode keeps the rendered length predictable, blocks RTL/zero-
// width attacks on display-name impersonation, and matches what every
// platform treats as "looks like a login".

fn is_valid_username(s: &str) -> bool {
    let len = s.len();
    if !(3..=32).contains(&len) {
        return false;
    }
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::is_valid_username;

    #[test]
    fn accepts_typical() {
        assert!(is_valid_username("alice"));
        assert!(is_valid_username("alice.smith"));
        assert!(is_valid_username("user_42"));
        assert!(is_valid_username("a-b-c"));
        assert!(is_valid_username("abc"));
        assert!(is_valid_username(&"a".repeat(32)));
    }

    #[test]
    fn rejects_too_short_or_long() {
        assert!(!is_valid_username(""));
        assert!(!is_valid_username("a"));
        assert!(!is_valid_username("ab"));
        assert!(!is_valid_username(&"a".repeat(33)));
    }

    #[test]
    fn rejects_disallowed_chars() {
        assert!(!is_valid_username("alice smith")); // space
        assert!(!is_valid_username("alice@home"));
        assert!(!is_valid_username("привет"));
        assert!(!is_valid_username("user/path"));
    }
}
