use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use matehub_common::snowflake;
use sqlx::PgPool;

use crate::auth::AuthUser;
use crate::db::rls::hub_connection;
use crate::models::Group;
use crate::models::group::{CreateGroup, UpdateGroup};
use matehub_common::perms::{bits, resolve_user_perms};

pub fn routes(pool: PgPool) -> Router {
    Router::new()
        .route("/hubs/{hub_id}/groups", get(list_groups).post(create_group))
        .route(
            "/hubs/{hub_id}/groups/{group_id}",
            axum::routing::patch(update_group).delete(delete_group),
        )
        .route(
            "/hubs/{hub_id}/groups/{group_id}/members/{user_id}",
            post(add_member_to_group).delete(remove_member_from_group),
        )
        .with_state(pool)
}

async fn list_groups(
    State(pool): State<PgPool>,
    Path(hub_id): Path<i64>,
) -> Result<Json<Vec<Group>>, StatusCode> {
    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let groups =
        sqlx::query_as::<_, Group>("SELECT * FROM groups WHERE hub_id = $1 ORDER BY position")
            .bind(hub_id)
            .fetch_all(&mut *conn)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(groups))
}

async fn create_group(
    State(pool): State<PgPool>,
    Path(hub_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<CreateGroup>,
) -> Result<(StatusCode, Json<Group>), StatusCode> {
    let caller = resolve_user_perms(&pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !caller.has(bits::MANAGE_ROLES) {
        return Err(StatusCode::FORBIDDEN);
    }

    let requested_bits = body.hub_permissions.unwrap_or(0);
    if requested_bits & !caller.grantable_bits() != 0 {
        return Err(StatusCode::FORBIDDEN);
    }

    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let max_pos: Option<i32> =
        sqlx::query_scalar("SELECT MAX(position) FROM groups WHERE hub_id = $1")
            .bind(hub_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let new_pos = max_pos.unwrap_or(0) + 1;

    let group = sqlx::query_as::<_, Group>(
        "INSERT INTO groups (id, hub_id, name, color, position, hub_permissions)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING *",
    )
    .bind(snowflake::next_id())
    .bind(hub_id)
    .bind(&body.name)
    .bind(&body.color)
    .bind(new_pos)
    .bind(requested_bits)
    .fetch_one(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((StatusCode::CREATED, Json(group)))
}

async fn update_group(
    State(pool): State<PgPool>,
    Path((hub_id, group_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(body): Json<UpdateGroup>,
) -> Result<Json<Group>, StatusCode> {
    let caller = resolve_user_perms(&pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let target = sqlx::query_as::<_, Group>(
        "SELECT * FROM groups WHERE id = $1 AND hub_id = $2",
    )
    .bind(group_id)
    .bind(hub_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    let is_protected = target.name == "admin" || target.name == "everyone";

    if is_protected {
        if body.position.is_some() || body.hub_permissions.is_some() {
            return Err(StatusCode::FORBIDDEN);
        }
    } else {
        if !caller.has(bits::MANAGE_ROLES) {
            return Err(StatusCode::FORBIDDEN);
        }
        if !caller.can_manage_position(target.position) {
            return Err(StatusCode::FORBIDDEN);
        }
        if let Some(new_bits) = body.hub_permissions {
            if new_bits & !caller.grantable_bits() != 0 {
                return Err(StatusCode::FORBIDDEN);
            }
        }
    }

    let group = sqlx::query_as::<_, Group>(
        "UPDATE groups SET
            name = COALESCE($3, name),
            color = COALESCE($4, color),
            position = COALESCE($5, position),
            hub_permissions = COALESCE($6, hub_permissions)
         WHERE id = $2 AND hub_id = $1
         RETURNING *",
    )
    .bind(hub_id)
    .bind(group_id)
    .bind(&body.name)
    .bind(&body.color)
    .bind(body.position)
    .bind(body.hub_permissions)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(group))
}

async fn delete_group(
    State(pool): State<PgPool>,
    Path((hub_id, group_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, StatusCode> {
    let caller = resolve_user_perms(&pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !caller.has(bits::MANAGE_ROLES) {
        return Err(StatusCode::FORBIDDEN);
    }

    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let target = sqlx::query_as::<_, Group>(
        "SELECT * FROM groups WHERE id = $1 AND hub_id = $2",
    )
    .bind(group_id)
    .bind(hub_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    if target.name == "admin" || target.name == "everyone" || target.is_default {
        return Err(StatusCode::FORBIDDEN);
    }
    if !caller.can_manage_position(target.position) {
        return Err(StatusCode::FORBIDDEN);
    }
    let caller_in_group: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM member_groups WHERE hub_id = $1 AND user_id = $2 AND group_id = $3)",
    )
    .bind(hub_id)
    .bind(auth.0.sub)
    .bind(group_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if caller_in_group {
        return Err(StatusCode::FORBIDDEN);
    }

    sqlx::query("DELETE FROM groups WHERE id = $1 AND hub_id = $2")
        .bind(group_id)
        .bind(hub_id)
        .execute(&mut *conn)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::NO_CONTENT)
}

async fn add_member_to_group(
    State(pool): State<PgPool>,
    Path((hub_id, group_id, user_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, StatusCode> {
    let caller = resolve_user_perms(&pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !caller.has(bits::MANAGE_ROLES) {
        return Err(StatusCode::FORBIDDEN);
    }

    let target_group_pos: i32 = sqlx::query_scalar(
        "SELECT position FROM groups WHERE id = $1 AND hub_id = $2",
    )
    .bind(group_id)
    .bind(hub_id)
    .fetch_optional(&pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    if !caller.can_manage_position(target_group_pos) {
        return Err(StatusCode::FORBIDDEN);
    }

    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    sqlx::query(
        "INSERT INTO member_groups (hub_id, user_id, group_id) VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
    )
    .bind(hub_id)
    .bind(user_id)
    .bind(group_id)
    .execute(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    crate::acl_publish::invalidate_user(hub_id, user_id).await;
    crate::member_events::member_groups_changed(hub_id, user_id);

    Ok(StatusCode::NO_CONTENT)
}

async fn remove_member_from_group(
    State(pool): State<PgPool>,
    Path((hub_id, group_id, user_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, StatusCode> {
    let caller = resolve_user_perms(&pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !caller.has(bits::MANAGE_ROLES) {
        return Err(StatusCode::FORBIDDEN);
    }

    #[derive(sqlx::FromRow)]
    struct GroupInfo { name: String, position: i32 }
    let target_group = sqlx::query_as::<_, GroupInfo>(
        "SELECT name, position FROM groups WHERE id = $1 AND hub_id = $2",
    )
    .bind(group_id)
    .bind(hub_id)
    .fetch_optional(&pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    if !caller.can_manage_position(target_group.position) {
        return Err(StatusCode::FORBIDDEN);
    }

    if target_group.name == "admin" {
        let is_creator: bool = sqlx::query_scalar(
            "SELECT COALESCE(creator_id = $2, false) FROM hubs WHERE id = $1",
        )
        .bind(hub_id)
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap_or(false);

        if is_creator {
            return Err(StatusCode::FORBIDDEN);
        }
    }

    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    sqlx::query("DELETE FROM member_groups WHERE hub_id = $1 AND user_id = $2 AND group_id = $3")
        .bind(hub_id)
        .bind(user_id)
        .bind(group_id)
        .execute(&mut *conn)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    crate::acl_publish::invalidate_user(hub_id, user_id).await;
    crate::member_events::member_groups_changed(hub_id, user_id);

    Ok(StatusCode::NO_CONTENT)
}
