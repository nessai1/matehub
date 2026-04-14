use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::rls::hub_connection;
use crate::models::Group;
use crate::models::group::{CreateGroup, UpdateGroup};

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
    Json(body): Json<CreateGroup>,
) -> Result<(StatusCode, Json<Group>), StatusCode> {
    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Position: append at end
    let max_pos: Option<i32> =
        sqlx::query_scalar("SELECT MAX(position) FROM groups WHERE hub_id = $1")
            .bind(hub_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let group = sqlx::query_as::<_, Group>(
        "INSERT INTO groups (hub_id, name, color, position)
         VALUES ($1, $2, $3, $4)
         RETURNING *",
    )
    .bind(hub_id)
    .bind(&body.name)
    .bind(&body.color)
    .bind(max_pos.unwrap_or(0) + 1)
    .fetch_one(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((StatusCode::CREATED, Json(group)))
}

async fn update_group(
    State(pool): State<PgPool>,
    Path((hub_id, group_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateGroup>,
) -> Result<Json<Group>, StatusCode> {
    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let group = sqlx::query_as::<_, Group>(
        "UPDATE groups SET
            name = COALESCE($3, name),
            color = COALESCE($4, color),
            position = COALESCE($5, position)
         WHERE id = $2 AND hub_id = $1
         RETURNING *",
    )
    .bind(hub_id)
    .bind(group_id)
    .bind(&body.name)
    .bind(&body.color)
    .bind(body.position)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(group))
}

async fn delete_group(
    State(pool): State<PgPool>,
    Path((hub_id, group_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, StatusCode> {
    let mut conn = hub_connection(&pool, hub_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Prevent deleting the default group
    let is_default: Option<bool> =
        sqlx::query_scalar("SELECT is_default FROM groups WHERE id = $1 AND hub_id = $2")
            .bind(group_id)
            .bind(hub_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match is_default {
        None => return Err(StatusCode::NOT_FOUND),
        Some(true) => return Err(StatusCode::FORBIDDEN),
        Some(false) => {}
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
) -> Result<StatusCode, StatusCode> {
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
) -> Result<StatusCode, StatusCode> {
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
