//! Temporary (guest) user lifecycle.
//!
//! Schema: temp users live in the same `users` table as everyone else, with
//! `user_type='temp'` and `users.expires_at` set. The `temp_users` side table
//! holds magic-link specifics (token, inviter, active WS session). This file
//! is responsible for keeping those two in sync — every code path mutating
//! one mutates the other in the same transaction.
//!
//! Lifecycle:
//!   1. POST /temp-users        → users (user_type=temp) + temp_users
//!   2. GET  /join/{token}      → hub_members + member_groups + active_session
//!   3. POST /temp-users/{id}/revoke
//!                              → temp_users.revoked_at; access dies on next
//!                                JWT verification once exp lapses (or sooner
//!                                if we add a Redis blacklist later).

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use matehub_common::snowflake;
use serde::Serialize;
use sqlx::PgPool;

use crate::auth::{AuthUser, Claims, create_token};
use crate::db::rls::hub_connection;
use crate::models::temp_user::{CreateTempUser, TempUserLink};
use matehub_common::perms::{bits, resolve_user_perms};

pub fn routes(pool: PgPool) -> Router {
    Router::new()
        .route(
            "/hubs/{hub_id}/temp-users",
            get(list_temp_users).post(create_temp_user),
        )
        .route(
            "/hubs/{hub_id}/temp-users/{user_id}/revoke",
            post(revoke_temp_user),
        )
        .route("/join/{token}", get(join_via_token))
        .with_state(pool)
}

/// What the admin UI needs about an active temp link. JOINs users for
/// nickname (display_name) and expires_at — both live there now.
#[derive(Serialize, sqlx::FromRow)]
struct TempUserView {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    user_id: i64,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    hub_id: i64,
    nickname: String,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    group_id: i64,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    created_by: i64,
    expires_at: DateTime<Utc>,
    active_session: Option<String>,
    created_at: DateTime<Utc>,
}

async fn list_temp_users(
    State(pool): State<PgPool>,
    Path(hub_id): Path<i64>,
) -> Result<Json<Vec<TempUserView>>, StatusCode> {
    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let rows = sqlx::query_as::<_, TempUserView>(
        "SELECT t.user_id, t.hub_id,
                u.display_name AS nickname,
                t.group_id, t.created_by,
                u.expires_at, t.active_session, t.created_at
         FROM temp_users t
         JOIN users u ON u.id = t.user_id
         WHERE t.hub_id = $1
           AND t.revoked_at IS NULL
           AND u.deleted_at IS NULL
           AND u.expires_at > now()
         ORDER BY t.created_at DESC",
    )
    .bind(hub_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(rows))
}

async fn create_temp_user(
    State(pool): State<PgPool>,
    Path(hub_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<CreateTempUser>,
) -> Result<(StatusCode, Json<TempUserLink>), StatusCode> {
    let caller = resolve_user_perms(&pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !caller.has(bits::CREATE_TEMP_LINKS) {
        return Err(StatusCode::FORBIDDEN);
    }

    let nickname = body.nickname.trim();
    if nickname.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let token = generate_token();
    let expires_at = Utc::now() + chrono::Duration::seconds(body.ttl_seconds);
    let user_id = snowflake::next_id();
    // Username on `users` is UNIQUE, but temp guests don't have a real login —
    // synthesise one from the user_id so two guests with the same nickname
    // don't collide. The user never sees this; display_name is what shows up
    // everywhere in the UI.
    let synthetic_username = format!("guest-{user_id}");

    let mut tx = pool
        .begin()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // RLS context for temp_users insert; users + hub_members aren't under RLS
    // but the side-effects later might be.
    sqlx::query(&format!("SET LOCAL app.current_hub_id = '{hub_id}'"))
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    sqlx::query(
        "INSERT INTO users (id, username, display_name, user_type, expires_at)
         VALUES ($1, $2, $3, 'temp', $4)",
    )
    .bind(user_id)
    .bind(&synthetic_username)
    .bind(nickname)
    .bind(expires_at)
    .execute(&mut *tx)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    sqlx::query(
        "INSERT INTO temp_users (user_id, hub_id, token, group_id, created_by)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(hub_id)
    .bind(&token)
    .bind(body.group_id)
    .bind(auth.0.sub)
    .execute(&mut *tx)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    tx.commit()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((
        StatusCode::CREATED,
        Json(TempUserLink {
            user_id,
            nickname: nickname.to_string(),
            invite_url: format!("/join/{token}"),
            expires_at,
        }),
    ))
}

async fn revoke_temp_user(
    State(pool): State<PgPool>,
    Path((hub_id, user_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, StatusCode> {
    let caller = resolve_user_perms(&pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !caller.has(bits::CREATE_TEMP_LINKS) {
        return Err(StatusCode::FORBIDDEN);
    }

    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Mark revoked and force the access window to slam shut at next token
    // verification: setting users.expires_at = now() makes any subsequent
    // login/refresh fail, and prevents this synth-user from re-walking the
    // link. (The currently-issued JWT keeps working until its own exp; a
    // Redis blacklist would close that gap if/when it matters.)
    let rows = sqlx::query(
        "WITH revoked AS (
             UPDATE temp_users
             SET revoked_at = now(), active_session = NULL
             WHERE user_id = $1 AND hub_id = $2 AND revoked_at IS NULL
             RETURNING user_id
         )
         UPDATE users SET expires_at = now()
         WHERE id = (SELECT user_id FROM revoked)",
    )
    .bind(user_id)
    .bind(hub_id)
    .execute(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .rows_affected();

    if rows == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Public endpoint: validate token, return hub info + JWT.
#[derive(Serialize)]
struct JoinResponse {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    hub_id: i64,
    hub_name: String,
    hub_slug: String,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    temp_user_id: i64,
    nickname: String,
    /// Real JWT (user_type="temp"), shared rails with chat/video services.
    session_token: String,
}

#[derive(sqlx::FromRow)]
struct JoinRow {
    user_id: i64,
    hub_id: i64,
    nickname: String,
    group_id: i64,
    expires_at: DateTime<Utc>,
}

async fn join_via_token(
    State(pool): State<PgPool>,
    Path(token): Path<String>,
) -> Result<Json<JoinResponse>, StatusCode> {
    // One round-trip for the lookup: temp metadata + the underlying users row.
    // u.expires_at being authoritative means we honour both natural expiry and
    // the slam-shut from revoke_temp_user that sets expires_at = now().
    let row = sqlx::query_as::<_, JoinRow>(
        "SELECT t.user_id, t.hub_id, u.display_name AS nickname,
                t.group_id, u.expires_at
         FROM temp_users t
         JOIN users u ON u.id = t.user_id
         WHERE t.token = $1
           AND t.revoked_at IS NULL
           AND u.deleted_at IS NULL
           AND u.expires_at > now()",
    )
    .bind(&token)
    .fetch_optional(&pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    let hub = sqlx::query_as::<_, crate::models::Hub>("SELECT * FROM hubs WHERE id = $1")
        .bind(row.hub_id)
        .fetch_one(&pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // First walk-through promotes them into hub_members + member_groups so
    // everything that JOINs hub_members (members-full, RLS, ACL checks in
    // chat-service) starts working. Subsequent walk-throughs (refresh,
    // re-open) are no-ops thanks to ON CONFLICT.
    let everyone_group_id: i64 = sqlx::query_scalar(
        "SELECT id FROM groups WHERE hub_id = $1 AND is_default = true LIMIT 1",
    )
    .bind(row.hub_id)
    .fetch_one(&pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    sqlx::query(&format!("SET LOCAL app.current_hub_id = '{}'", row.hub_id))
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let inserted_member: bool = sqlx::query_scalar(
        "INSERT INTO hub_members (hub_id, user_id, role) VALUES ($1, $2, 'guest')
         ON CONFLICT (hub_id, user_id) DO NOTHING
         RETURNING true",
    )
    .bind(row.hub_id)
    .bind(row.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .unwrap_or(false);

    for gid in [everyone_group_id, row.group_id] {
        sqlx::query(
            "INSERT INTO member_groups (hub_id, user_id, group_id) VALUES ($1, $2, $3)
             ON CONFLICT DO NOTHING",
        )
        .bind(row.hub_id)
        .bind(row.user_id)
        .bind(gid)
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }

    let new_session = uuid::Uuid::new_v4().to_string();
    sqlx::query("UPDATE temp_users SET active_session = $2 WHERE user_id = $1")
        .bind(row.user_id)
        .bind(&new_session)
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    tx.commit()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if inserted_member {
        // Tell connected presence WS clients to refetch the roster — pending
        // invite vanishes from one section, real member appears in another.
        crate::member_events::member_joined(row.hub_id, row.user_id);
    }

    let now = Utc::now().timestamp();
    let claims = Claims {
        sub: row.user_id,
        username: row.nickname.clone(),
        user_type: "temp".into(),
        hub_id: row.hub_id,
        groups: vec![everyone_group_id, row.group_id],
        iat: now,
        exp: row.expires_at.timestamp(),
    };
    let session_token = create_token(&claims).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(JoinResponse {
        hub_id: hub.id,
        hub_name: hub.name,
        hub_slug: hub.slug,
        temp_user_id: row.user_id,
        nickname: row.nickname,
        session_token,
    }))
}

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
