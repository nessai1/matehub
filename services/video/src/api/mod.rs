pub mod sessions;
pub mod ws;

use axum::{Json, Router};

use crate::state::AppState;

pub fn routes(state: AppState) -> Router {
    Router::new()
        .merge(sessions::routes())
        .merge(ws::routes())
        .route("/health", axum::routing::get(|| async { "ok" }))
        .route("/version", axum::routing::get(version_handler))
        .with_state(state)
}

async fn version_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "service": "video",
        "version": option_env!("MATEHUB_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
    }))
}
