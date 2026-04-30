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
    routing::{get, post},
};
use matehub_common::{
    perms::{bits, resolve_user_perms},
    snowflake,
};
use sqlx::PgPool;

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

async fn create_invitation(
    State(pool): State<PgPool>,
    Path(hub_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<CreateInvitationRequest>,
) -> Result<(StatusCode, Json<InvitationLink>), StatusCode> {
    if body.username.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let caller = resolve_user_perms(&pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !caller.has(bits::INVITE_PERMANENT) {
        return Err(StatusCode::FORBIDDEN);
    }

    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let token = generate_token();

    sqlx::query(
        "INSERT INTO invitations (id, hub_id, token, username, email, created_by)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(snowflake::next_id())
    .bind(hub_id)
    .bind(&token)
    .bind(&body.username)
    .bind(&body.email)
    .bind(auth.0.sub)
    .execute(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((
        StatusCode::CREATED,
        Json(InvitationLink {
            token: token.clone(),
            username: body.username,
            email: body.email,
            invite_url: format!("/invite/{token}"),
        }),
    ))
}

// ── Public preview ──────────────────────────────────────────────

async fn get_invitation(
    State(pool): State<PgPool>,
    Path(token): Path<String>,
) -> Result<Json<InvitationPreview>, StatusCode> {
    // Public endpoint: no RLS context (invitations are looked up by an
    // unguessable token, not by hub_id).
    let row: Option<(String, Option<String>, Option<chrono::DateTime<chrono::Utc>>, String)> =
        sqlx::query_as(
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
    let invitation: Option<Invitation> = sqlx::query_as::<_, Invitation>(
        "SELECT * FROM invitations WHERE token = $1 FOR UPDATE",
    )
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

    sqlx::query(
        "INSERT INTO hub_members (hub_id, user_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(hub_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Drop the new user into the hub's default group ("everyone").
    let everyone_group_id: i64 = sqlx::query_scalar(
        "SELECT id FROM groups WHERE hub_id = $1 AND is_default = true LIMIT 1",
    )
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

    sqlx::query(
        "UPDATE invitations SET used_at = now(), used_by = $1 WHERE id = $2",
    )
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

// ── Token gen (same scheme as temp_users) ───────────────────────

fn generate_token() -> String {
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
