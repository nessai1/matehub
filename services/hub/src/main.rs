mod acl_publish;
mod api;
mod auth;
mod db;
mod models;
mod presence;
mod storage;
mod voice_occupancy_bus;

use std::sync::Arc;

use anyhow::Result;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::from_path("../../.env")
        .or_else(|_| dotenvy::from_path(".env"))
        .or_else(|_| dotenvy::dotenv().map(|_| ()));

    matehub_common::observability::init_tracing("info,sqlx=warn");

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://matehub:matehub-dev@localhost:5432/matehub".into());
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3002);
    let dev_mode = std::env::var("DEV_MODE")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(true);

    // Snowflake generator: every service in the hub cluster shares the same
    // ID space. Must run before any code that calls `snowflake::next_id()`
    // (seed, channel/group creation, refresh-token insert, etc).
    matehub_common::snowflake::init();

    let pool = db::connect(&database_url).await?;
    db::migrate(&pool).await?;

    if dev_mode {
        db::seed::run_dev_seed(&pool).await?;
    }

    // S3 storage (optional)
    let s3 = if std::env::var("S3_ACCESS_KEY_ID").is_ok() {
        Some(Arc::new(storage::S3Storage::from_env("S3_BUCKET_HUB_ASSETS").await))
    } else {
        tracing::warn!("S3_ACCESS_KEY_ID not set, avatar uploads disabled");
        None
    };

    // Redis for presence (optional)
    let redis = presence::connect_redis().await;

    // Presence broadcast bus — voice-occupancy NATS subscriber pushes into
    // this, every WS client subscribes to it. `256` is the per-subscriber
    // ring buffer: bursty enough that a briefly-blocked WS doesn't lose
    // events, small enough that memory is bounded at N_clients × 256.
    let (events_tx, _) = tokio::sync::broadcast::channel(256);

    // Start NATS → Redis/WS bridge in the background. Swallows failures so
    // the hub still comes up when NATS is down in a dev box.
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    voice_occupancy_bus::spawn(&nats_url, redis.clone(), events_tx.clone()).await;

    // Single NATS client for outbound ACL invalidations. None means publishes
    // become no-ops; the hub still works, just without cache-busting.
    let nats_for_acl = match async_nats::connect(&nats_url).await {
        Ok(c) => {
            tracing::info!(%nats_url, "NATS connected (acl_publish)");
            Some(c)
        }
        Err(e) => {
            tracing::warn!(error = %e, "acl_publish NATS connect failed, invalidations disabled");
            None
        }
    };
    acl_publish::init(nats_for_acl);

    // SSO state. HUB_ID comes from ENV in SaaS (set by general at
    // provisioning) or falls back to the seed DEV_HUB_ID in dev.
    // GENERAL_URL unset ⇒ on-prem mode — the SSO endpoint returns 501.
    let hub_id = std::env::var("HUB_ID")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(db::seed::DEV_HUB_ID);
    let general_url = std::env::var("GENERAL_URL").ok();
    let sso_state = api::sso::SsoState {
        pool: pool.clone(),
        general_url,
        hub_id,
    };

    let (metrics_layer, metrics_handle) =
        matehub_common::observability::metrics_layer_and_handle();

    // SPA bundle: when STATIC_DIR is set and exists, hub serves the Vite
    // bundle for any route the API didn't match. Unknown paths fall through
    // to index.html so client-side react-router handles deep links.
    // In dev (cargo run) STATIC_DIR is unset → directory doesn't exist →
    // ServeDir 404s, frontend is served separately by `vite dev` on :3000.
    let static_dir = std::env::var("STATIC_DIR").unwrap_or_else(|_| "/srv/dist".into());
    let index_path = format!("{static_dir}/index.html");
    let spa_fallback =
        ServeDir::new(&static_dir).not_found_service(ServeFile::new(&index_path));
    if std::path::Path::new(&static_dir).is_dir() {
        tracing::info!(%static_dir, "serving SPA bundle as fallback");
    } else {
        tracing::warn!(%static_dir, "STATIC_DIR not present, SPA fallback will 404");
    }

    // /health is already registered inside api::routes(); don't duplicate it.
    let app = api::routes(pool, s3, redis, events_tx, dev_mode, sso_state)
        .route(
            "/metrics",
            axum::routing::get({
                let h = metrics_handle.clone();
                move || async move { h.render() }
            }),
        )
        .fallback_service(spa_fallback)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .layer(metrics_layer);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!(%port, %dev_mode, "matehub-hub started");

    axum::serve(listener, app).await?;
    Ok(())
}
