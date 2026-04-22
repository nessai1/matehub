use std::sync::Arc;

use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use serde::Serialize;

use crate::api::extract::Authed;
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/dashboard/hubs", get(my_hubs))
}

#[derive(Serialize)]
struct HubSummary {
    id: i64,
    slug: String,
    name: String,
    status: String,
    role: String,
}

async fn my_hubs(
    State(state): State<Arc<AppState>>,
    Authed(claims): Authed,
) -> Result<Json<Vec<HubSummary>>, (StatusCode, String)> {
    let rows: Vec<(i64, String, String, String, String)> = sqlx::query_as(
        r#"
        SELECT h.id, h.slug, h.name, h.status, hm.role
        FROM hubs h
        JOIN hub_members hm ON hm.hub_id = h.id
        WHERE hm.account_id = $1
        ORDER BY h.created_at DESC
        "#,
    )
    .bind(claims.sub)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let hubs = rows
        .into_iter()
        .map(|(id, slug, name, status, role)| HubSummary {
            id,
            slug,
            name,
            status,
            role,
        })
        .collect();

    Ok(Json(hubs))
}
