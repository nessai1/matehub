pub mod sessions;
pub mod ws;

use axum::Router;

use crate::state::AppState;

pub fn routes(state: AppState) -> Router {
    Router::new()
        .merge(sessions::routes())
        .merge(ws::routes())
        .route("/health", axum::routing::get(|| async { "ok" }))
        .with_state(state)
}
