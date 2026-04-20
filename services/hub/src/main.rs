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
use tower_http::trace::TraceLayer;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::from_path("../../.env")
        .or_else(|_| dotenvy::from_path(".env"))
        .or_else(|_| dotenvy::dotenv().map(|_| ()));

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://matehub:matehub-dev@localhost:5432/matehub".into());
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3002);
    let dev_mode = std::env::var("DEV_MODE")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(true);

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

    let app = api::routes(pool, s3, redis, events_tx, dev_mode)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .route("/health", axum::routing::get(|| async { "ok" }));

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!(%port, %dev_mode, "matehub-hub started");

    axum::serve(listener, app).await?;
    Ok(())
}
