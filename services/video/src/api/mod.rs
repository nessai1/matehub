pub mod sessions;
pub mod ws;

#[cfg(feature = "profiling")]
pub mod profiling;

use axum::{Json, Router};

use crate::state::AppState;

pub fn routes(state: AppState) -> Router {
    let router = Router::new()
        .merge(sessions::routes())
        .merge(ws::routes());

    // The `mut` is conditional — keeps cargo from warning when the
    // profiling feature is off and nothing else mutates `router`.
    #[cfg(feature = "profiling")]
    let router = router.merge(profiling::routes());

    router
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
