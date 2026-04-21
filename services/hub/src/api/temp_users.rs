use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use chrono::Utc;
use matehub_common::snowflake;
use sqlx::PgPool;

use crate::auth::AuthUser;
use crate::api::auth_check::resolve_user_perms;
use crate::db::rls::hub_connection;
use crate::models::TempUser;
use crate::models::temp_user::{CreateTempUser, TempUserLink};
use crate::models::permission::bits;

pub fn routes(pool: PgPool) -> Router {
    Router::new()
        .route(
            "/hubs/{hub_id}/temp-users",
            get(list_temp_users).post(create_temp_user),
        )
        .route(
            "/hubs/{hub_id}/temp-users/{temp_user_id}/revoke",
            post(revoke_temp_user),
        )
        .route("/join/{token}", get(join_via_token))
        .with_state(pool)
}

async fn list_temp_users(
    State(pool): State<PgPool>,
    Path(hub_id): Path<i64>,
) -> Result<Json<Vec<TempUser>>, StatusCode> {
    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let users = sqlx::query_as::<_, TempUser>(
        "SELECT * FROM temp_users
         WHERE hub_id = $1 AND revoked_at IS NULL AND expires_at > now()
         ORDER BY created_at DESC",
    )
    .bind(hub_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(users))
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

    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let token = generate_token();
    let expires_at = Utc::now() + chrono::Duration::seconds(body.ttl_seconds);
    let created_by = auth.0.sub;

    let temp_user = sqlx::query_as::<_, TempUser>(
        "INSERT INTO temp_users (id, hub_id, token, nickname, group_id, created_by, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING *",
    )
    .bind(snowflake::next_id())
    .bind(hub_id)
    .bind(&token)
    .bind(&body.nickname)
    .bind(body.group_id)
    .bind(created_by)
    .bind(expires_at)
    .fetch_one(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((
        StatusCode::CREATED,
        Json(TempUserLink {
            id: temp_user.id,
            nickname: temp_user.nickname,
            invite_url: format!("/join/{token}"),
            expires_at: temp_user.expires_at,
        }),
    ))
}

async fn revoke_temp_user(
    State(pool): State<PgPool>,
    Path((hub_id, temp_user_id)): Path<(i64, i64)>,
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

    let rows = sqlx::query(
        "UPDATE temp_users SET revoked_at = now(), active_session = NULL
         WHERE id = $1 AND hub_id = $2 AND revoked_at IS NULL",
    )
    .bind(temp_user_id)
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

/// Public endpoint: validate token, return hub info for redirect.
#[derive(serde::Serialize)]
struct JoinResponse {
    hub_id: i64,
    hub_name: String,
    hub_slug: String,
    temp_user_id: i64,
    nickname: String,
    /// Session token for WS connections to video/chat services.
    session_token: String,
}

async fn join_via_token(
    State(pool): State<PgPool>,
    Path(token): Path<String>,
) -> Result<Json<JoinResponse>, StatusCode> {
    let temp_user = sqlx::query_as::<_, TempUser>(
        "SELECT * FROM temp_users
         WHERE token = $1 AND revoked_at IS NULL AND expires_at > now()",
    )
    .bind(&token)
    .fetch_optional(&pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    let hub = sqlx::query_as::<_, crate::models::Hub>("SELECT * FROM hubs WHERE id = $1")
        .bind(temp_user.hub_id)
        .fetch_one(&pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let new_session = uuid::Uuid::new_v4().to_string();
    sqlx::query("UPDATE temp_users SET active_session = $2 WHERE id = $1")
        .bind(temp_user.id)
        .bind(&new_session)
        .execute(&pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(JoinResponse {
        hub_id: hub.id,
        hub_name: hub.name,
        hub_slug: hub.slug,
        temp_user_id: temp_user.id,
        nickname: temp_user.nickname,
        // TODO: issue a real JWT with user_type="temp"; stub preserves the prior shape.
        session_token: format!("temp-{}-{}", temp_user.id, token),
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
