use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::api::auth_check::{resolve_user_perms, resolve_target_position};
use crate::db::rls::hub_connection;
use crate::models::Group;
use crate::models::group::{CreateGroup, UpdateGroup};
use crate::models::permission::bits;

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
    Path(hub_id): Path<Uuid>,
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
    Path(hub_id): Path<Uuid>,
    auth: AuthUser,
    Json(body): Json<CreateGroup>,
) -> Result<(StatusCode, Json<Group>), StatusCode> {
    // Must have MANAGE_ROLES
    let caller = resolve_user_perms(&pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !caller.has(bits::MANAGE_ROLES) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Can only grant bits that caller has
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

    // New group is always below caller (higher position number)
    let new_pos = max_pos.unwrap_or(0) + 1;

    let group = sqlx::query_as::<_, Group>(
        "INSERT INTO groups (hub_id, name, color, position, hub_permissions)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING *",
    )
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
    Path((hub_id, group_id)): Path<(Uuid, Uuid)>,
    auth: AuthUser,
    Json(body): Json<UpdateGroup>,
) -> Result<Json<Group>, StatusCode> {
    let caller = resolve_user_perms(&pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Fetch target group
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
        // admin/everyone: only name and color allowed
        if body.position.is_some() || body.hub_permissions.is_some() {
            return Err(StatusCode::FORBIDDEN);
        }
    } else {
        // Must have MANAGE_ROLES and be above the target group
        if !caller.has(bits::MANAGE_ROLES) {
            return Err(StatusCode::FORBIDDEN);
        }
        if !caller.can_manage_position(target.position) {
            return Err(StatusCode::FORBIDDEN);
        }
        // Can only grant bits that caller has
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
    Path((hub_id, group_id)): Path<(Uuid, Uuid)>,
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

    // Can't delete admin or everyone
    if target.name == "admin" || target.name == "everyone" || target.is_default {
        return Err(StatusCode::FORBIDDEN);
    }
    // Can only delete groups below caller
    if !caller.can_manage_position(target.position) {
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
    Path((hub_id, group_id, user_id)): Path<(Uuid, Uuid, Uuid)>,
    auth: AuthUser,
) -> Result<StatusCode, StatusCode> {
    let caller = resolve_user_perms(&pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !caller.has(bits::MANAGE_ROLES) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Fetch target group position
    let target_group_pos: i32 = sqlx::query_scalar(
        "SELECT position FROM groups WHERE id = $1 AND hub_id = $2",
    )
    .bind(group_id)
    .bind(hub_id)
    .fetch_optional(&pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    // Can only assign groups below caller
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

    Ok(StatusCode::NO_CONTENT)
}

async fn remove_member_from_group(
    State(pool): State<PgPool>,
    Path((hub_id, group_id, user_id)): Path<(Uuid, Uuid, Uuid)>,
    auth: AuthUser,
) -> Result<StatusCode, StatusCode> {
    let caller = resolve_user_perms(&pool, hub_id, auth.0.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !caller.has(bits::MANAGE_ROLES) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Fetch target group
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

    // Can only manage groups below caller
    if !caller.can_manage_position(target_group.position) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Can't remove hub creator from admin
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

    Ok(StatusCode::NO_CONTENT)
}
