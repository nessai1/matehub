mod api;
mod auth;
mod config;
mod db;
mod mailer;
mod provisioning;
mod state;

use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::config::Config;
use crate::state::AppState;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::from_path("../../.env")
        .or_else(|_| dotenvy::from_path(".env"))
        .or_else(|_| dotenvy::dotenv().map(|_| ()));

    matehub_common::observability::init_tracing("info,sqlx=warn");

    // Snowflake generator for account/hub/outbox IDs. Must run before any
    // code path that calls `snowflake::next_id()`.
    matehub_common::snowflake::init();

    let config = Config::from_env()?;
    let pool = db::connect(&config.database_url).await?;
    db::migrate(&pool).await?;

    let mail_transport = mailer::transport::from_env();
    let k8s = provisioning::K8sClient::try_connect().await;

    let state = Arc::new(AppState {
        pool: pool.clone(),
        config: config.clone(),
        mail_transport: mail_transport.clone(),
        k8s,
    });

    // Spawn outbox worker — polls mail_outbox, sends via transport.
    // Lives for the lifetime of the process; crash isolates via tokio task.
    tokio::spawn(mailer::worker::run(
        pool.clone(),
        mail_transport.clone(),
        config.mail_from.clone(),
    ));

    let (metrics_layer, metrics_handle) =
        matehub_common::observability::metrics_layer_and_handle();

    let app = Router::new()
        .merge(api::auth::routes())
        .merge(api::account::routes())
        .merge(api::dashboard::routes())
        .merge(api::hubs::routes())
        .merge(api::landing::routes())
        .merge(api::sso::routes())
        .merge(api::webhooks::routes())
        .with_state(state)
        .route(
            "/metrics",
            axum::routing::get({
                let h = metrics_handle.clone();
                move || async move { h.render() }
            }),
        )
        .route("/version", axum::routing::get(version_handler))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .layer(metrics_layer);

    let addr = format!("0.0.0.0:{}", config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "matehub-general listening");
    axum::serve(listener, app).await?;

    Ok(())
}

async fn version_handler() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "service": "general",
        "version": option_env!("MATEHUB_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
    }))
}
