// GET /config.json -- runtime config for the SPA bundle.
//
// The Vite bundle is built once, deployment-agnostic, and fetches this
// endpoint on boot before initialising the React tree. Anything that
// varies per deployment (TURN credentials, app version) lives here, not
// in `import.meta.env`. API URLs are NOT here -- they're hardcoded as
// same-origin paths (`/api/hub`, `/api/chat`, `/api/video`) because the
// Caddy routing layout is an architectural invariant, not config.

use axum::{Json, Router, routing::get};
use serde::Serialize;

#[derive(Serialize)]
pub struct RuntimeConfig {
    pub turn: TurnConfig,
    pub app_version: Option<String>,
}

#[derive(Serialize)]
pub struct TurnConfig {
    pub url: Option<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

async fn config() -> Json<RuntimeConfig> {
    Json(RuntimeConfig {
        turn: TurnConfig {
            url: std::env::var("TURN_URL").ok(),
            username: std::env::var("TURN_USERNAME").ok(),
            credential: std::env::var("TURN_PASSWORD").ok(),
        },
        app_version: option_env!("MATEHUB_VERSION").map(String::from),
    })
}

pub fn routes() -> Router {
    Router::new().route("/config.json", get(config))
}
