//! Permanent-account invitations.
//!
//! Companion to `temp_users.rs` (guest links): one row per pending
//! permanent account. The admin pre-allocates the `username`, the
//! invitee fills in their `display_name` + password by visiting
//! `/invite/{token}`. Single-use — `used_at` flips on accept and the
//! link returns 410 Gone forever after.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use email_address::EmailAddress;
use matehub_common::{
    perms::{bits, resolve_user_perms},
    snowflake,
};
use serde::Serialize;
use sqlx::PgPool;
use std::str::FromStr;

use crate::api::auth_api::{ACCESS_TOKEN_TTL_SECS, issue_access_token, issue_refresh_token};
use crate::auth::AuthUser;
use crate::db::rls::hub_connection;
use crate::models::Invitation;
use crate::models::invitation::{
    AcceptInvitationRequest, CreateInvitationRequest, InvitationLink, InvitationPreview,
};

pub fn routes(pool: PgPool) -> Router {
    Router::new()
        .route("/v1/hubs/{hub_id}/invitations", post(create_invitation))
        .route("/v1/invitations/{token}", get(get_invitation))
        .route("/v1/invitations/{token}/accept", post(accept_invitation))
        .with_state(pool)
}

// ── Create (admin) ──────────────────────────────────────────────

/// Discriminated error body so the FE can tell *which* field is wrong
/// without re-parsing prose. Serializes as `{"error": "username_taken"}`.
#[derive(Serialize)]
struct CreateInviteError {
    error: &'static str,
}

impl CreateInviteError {
    fn into_response(self, status: StatusCode) -> Response {
        (status, Json(self)).into_response()
    }
}

async fn create_invitation(
    State(pool): State<PgPool>,
    Path(hub_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<CreateInvitationRequest>,
) -> Result<(StatusCode, Json<InvitationLink>), Response> {
    let username = body.username.trim();
    if username.is_empty() {
        return Err(CreateInviteError {
            error: "username_required",
        }
        .into_response(StatusCode::BAD_REQUEST));
    }

    // Email is optional; normalise empty string to None so the rest of the
    // pipeline doesn't have to keep checking both shapes.
    let email = body
        .email
        .as_deref()
        .map(|e| e.trim())
        .filter(|e| !e.is_empty());

    if let Some(addr) = email
        && !is_valid_email(addr)
    {
        return Err(CreateInviteError {
            error: "invalid_email",
        }
        .into_response(StatusCode::BAD_REQUEST));
    }

    let caller = resolve_user_perms(&pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| internal_error())?;
    if !caller.has(bits::INVITE_PERMANENT) {
        return Err(CreateInviteError { error: "forbidden" }.into_response(StatusCode::FORBIDDEN));
    }

    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| internal_error())?;

    // Pre-flight uniqueness checks. Race-loser still hits the users.username
    // UNIQUE on accept (and the partial unique index on email), but checking
    // here saves the inviter from handing out a link that's already
    // doomed — the common case where an admin types a login that's
    // already in use.
    let username_taken_in_users: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE username = $1)")
            .bind(username)
            .fetch_one(&mut *conn)
            .await
            .map_err(|_| internal_error())?;
    if username_taken_in_users {
        return Err(CreateInviteError {
            error: "username_taken",
        }
        .into_response(StatusCode::CONFLICT));
    }

    let username_pending: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM invitations
                        WHERE hub_id = $1 AND username = $2 AND used_at IS NULL)",
    )
    .bind(hub_id)
    .bind(username)
    .fetch_one(&mut *conn)
    .await
    .map_err(|_| internal_error())?;
    if username_pending {
        return Err(CreateInviteError {
            error: "username_pending",
        }
        .into_response(StatusCode::CONFLICT));
    }

    if let Some(addr) = email {
        let email_taken_in_users: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE email = $1)")
                .bind(addr)
                .fetch_one(&mut *conn)
                .await
                .map_err(|_| internal_error())?;
        if email_taken_in_users {
            return Err(CreateInviteError {
                error: "email_taken",
            }
            .into_response(StatusCode::CONFLICT));
        }

        let email_pending: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM invitations
                            WHERE hub_id = $1 AND email = $2 AND used_at IS NULL)",
        )
        .bind(hub_id)
        .bind(addr)
        .fetch_one(&mut *conn)
        .await
        .map_err(|_| internal_error())?;
        if email_pending {
            return Err(CreateInviteError {
                error: "email_pending",
            }
            .into_response(StatusCode::CONFLICT));
        }
    }

    let token = generate_token();

    sqlx::query(
        "INSERT INTO invitations (id, hub_id, token, username, email, created_by)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(snowflake::next_id())
    .bind(hub_id)
    .bind(&token)
    .bind(username)
    .bind(email)
    .bind(auth.0.sub)
    .execute(&mut *conn)
    .await
    .map_err(|_| internal_error())?;

    Ok((
        StatusCode::CREATED,
        Json(InvitationLink {
            token: token.clone(),
            username: username.to_string(),
            email: email.map(str::to_string),
            invite_url: format!("/invite/{token}"),
        }),
    ))
}

fn internal_error() -> Response {
    CreateInviteError { error: "internal" }.into_response(StatusCode::INTERNAL_SERVER_ERROR)
}

// ── Public preview ──────────────────────────────────────────────

async fn get_invitation(
    State(pool): State<PgPool>,
    Path(token): Path<String>,
) -> Result<Json<InvitationPreview>, StatusCode> {
    // Public endpoint: no RLS context (invitations are looked up by an
    // unguessable token, not by hub_id).
    type InvitationRow = (
        String,                                // i.username
        Option<String>,                        // i.email
        Option<chrono::DateTime<chrono::Utc>>, // i.used_at
        String,                                // h.name
    );
    let row: Option<InvitationRow> = sqlx::query_as(
        "SELECT i.username, i.email, i.used_at, h.name
             FROM invitations i
             JOIN hubs h ON h.id = i.hub_id
             WHERE i.token = $1",
    )
    .bind(&token)
    .fetch_optional(&pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let (username, email, used_at, hub_name) = row.ok_or(StatusCode::NOT_FOUND)?;

    if used_at.is_some() {
        return Err(StatusCode::GONE);
    }

    Ok(Json(InvitationPreview {
        hub_name,
        username,
        email,
    }))
}

// ── Accept (public, sets up the user) ───────────────────────────

#[derive(serde::Serialize)]
struct AcceptResponse {
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

async fn accept_invitation(
    State(pool): State<PgPool>,
    Path(token): Path<String>,
    Json(body): Json<AcceptInvitationRequest>,
) -> Result<Json<AcceptResponse>, StatusCode> {
    if body.password.is_empty() || body.display_name.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut tx = pool
        .begin()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Lock the row so two concurrent accepts can't both win.
    let invitation: Option<Invitation> =
        sqlx::query_as::<_, Invitation>("SELECT * FROM invitations WHERE token = $1 FOR UPDATE")
            .bind(&token)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let invitation = invitation.ok_or(StatusCode::NOT_FOUND)?;
    if invitation.used_at.is_some() {
        return Err(StatusCode::GONE);
    }

    let hub_id = invitation.hub_id;

    // Need RLS context for member_groups + (later) any hub-scoped reads.
    sqlx::query(&format!("SET LOCAL app.current_hub_id = '{hub_id}'"))
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let password_hash = bcrypt::hash(&body.password, bcrypt::DEFAULT_COST)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let user_id = snowflake::next_id();

    // Username UNIQUE → 409 if someone else snagged it between create/accept.
    sqlx::query(
        "INSERT INTO users (id, username, display_name, email, password_hash)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(&invitation.username)
    .bind(&body.display_name)
    .bind(&invitation.email)
    .bind(&password_hash)
    .execute(&mut *tx)
    .await
    .map_err(|e| match e.as_database_error().and_then(|e| e.code()) {
        Some(code) if code == "23505" => StatusCode::CONFLICT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    })?;

    sqlx::query("INSERT INTO hub_members (hub_id, user_id, role) VALUES ($1, $2, 'member')")
        .bind(hub_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Drop the new user into the hub's default group ("everyone").
    let everyone_group_id: i64 =
        sqlx::query_scalar("SELECT id FROM groups WHERE hub_id = $1 AND is_default = true LIMIT 1")
            .bind(hub_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let group_ids: Vec<i64> = if let Some(invite_group_id) = invitation.group_id {
        vec![everyone_group_id, invite_group_id]
    } else {
        vec![everyone_group_id]
    };

    for group_id in &group_ids {
        sqlx::query(
            "INSERT INTO member_groups (hub_id, user_id, group_id) VALUES ($1, $2, $3)
             ON CONFLICT DO NOTHING",
        )
        .bind(hub_id)
        .bind(user_id)
        .bind(group_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }

    sqlx::query("UPDATE invitations SET used_at = now(), used_by = $1 WHERE id = $2")
        .bind(user_id)
        .bind(invitation.id)
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    tx.commit()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Notify already-connected presence WS clients that the roster grew.
    // Their SWR cache for `members-full` will be invalidated and re-fetched.
    crate::member_events::member_joined(hub_id, user_id);

    let hub_slug: String = sqlx::query_scalar("SELECT slug FROM hubs WHERE id = $1")
        .bind(hub_id)
        .fetch_one(&pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let access_token = issue_access_token(user_id, &invitation.username, hub_id, &group_ids)?;
    let refresh_token = issue_refresh_token(&pool, user_id, hub_id).await?;

    tracing::info!(
        username = %invitation.username,
        %hub_id,
        invitation_id = invitation.id,
        "permanent invitation accepted"
    );

    Ok(Json(AcceptResponse {
        access_token,
        refresh_token,
        expires_in: ACCESS_TOKEN_TTL_SECS,
        user_id,
        username: invitation.username,
        display_name: body.display_name,
        hub_id,
        hub_slug,
    }))
}

// ── Email validation ────────────────────────────────────────────
//
// RFC 5321 technically allows bare hostnames as email domains
// (`alice@localhost` parses fine), so the `email_address` crate alone
// would let "wera@dad" through. For our UX — admin types a coworker's
// real email — we add one extra rule on top of the parser:
//
//   * the domain must contain at least one dot, with that dot not at
//     either end of the domain.
//
// That's it. Anything stricter (TLD length, character class) starts
// rejecting valid-but-unfamiliar shapes (`name@host.d`, punycode IDNs,
// etc.) and turns into a "validator vs reality" tug-of-war. Format
// validation is a shape check; deliverability is DNS's problem.

pub(super) fn is_valid_email(s: &str) -> bool {
    if EmailAddress::from_str(s).is_err() {
        return false;
    }

    // EmailAddress accepted it, so we know there's exactly one '@'.
    let Some(domain) = s.rsplit('@').next() else {
        return false;
    };

    domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

// ── Token gen (same scheme as temp_users) ───────────────────────

pub(super) fn generate_token() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let bytes: [u8; 24] = rng.random();
    base64_url_encode(&bytes)
}

fn base64_url_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut result = String::with_capacity(data.len() * 4 / 3 + 1);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        }
        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::is_valid_email;

    #[test]
    fn accepts_normal_addresses() {
        assert!(is_valid_email("alice@example.com"));
        assert!(is_valid_email("alice.smith@example.co.uk"));
        assert!(is_valid_email("alice+tag@example.com"));
        assert!(is_valid_email("a@b.io"));
        assert!(is_valid_email("with-dash@sub.domain.example"));
    }

    #[test]
    fn accepts_unusual_but_structurally_valid() {
        // Single-char TLDs aren't in the public DNS root, but the address
        // is structurally fine. We're a format validator, not a registry.
        assert!(is_valid_email("adad@dadad.d"));
        // Punycode IDN — perfectly real, just has a hyphen.
        assert!(is_valid_email("alice@xn--mxail-6qa.de"));
    }

    #[test]
    fn rejects_obvious_garbage() {
        assert!(!is_valid_email(""));
        assert!(!is_valid_email("not-an-email"));
        assert!(!is_valid_email("@"));
        assert!(!is_valid_email("@example.com"));
        assert!(!is_valid_email("alice@"));
        assert!(!is_valid_email("alice@@example.com"));
        assert!(!is_valid_email("alice example.com"));
    }

    #[test]
    fn rejects_missing_tld() {
        // The exact case that motivated this validator:
        assert!(!is_valid_email("wera@dad"));
        // Common typo — forgetting the TLD on a known provider:
        assert!(!is_valid_email("alice@gmail"));
        // A bare hostname that RFC 5321 technically accepts but we don't:
        assert!(!is_valid_email("test@localhost"));
    }

    #[test]
    fn rejects_dots_at_domain_edges() {
        assert!(!is_valid_email("alice@.com"));
        assert!(!is_valid_email("alice@example."));
        assert!(!is_valid_email("alice@.example.com"));
    }

    #[test]
    fn rejects_whitespace() {
        assert!(!is_valid_email(" alice@example.com"));
        assert!(!is_valid_email("alice@example.com "));
        assert!(!is_valid_email("alice@ example.com"));
    }
}
