#![allow(dead_code, clippy::collapsible_if)]

mod api;
mod attachment;
mod attachments;
mod transcode;
mod auth;
mod data_service;
mod db;
mod fanout;
mod gateway;
mod models;
mod read_state;
mod session;

use std::sync::Arc;

use anyhow::Result;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use api::AppState;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::from_path("../../.env")
        .or_else(|_| dotenvy::from_path(".env"))
        .or_else(|_| dotenvy::dotenv().map(|_| ()));

    matehub_common::observability::init_tracing("info,scylla=warn");

    let port: u16 = std::env::var("CHAT_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3003);

    let scylla_url =
        std::env::var("SCYLLA_URL").unwrap_or_else(|_| "127.0.0.1:9042".into());
    let nats_url =
        std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());

    // Initialize Snowflake ID generator
    matehub_common::snowflake::init();

    // Connect to ScyllaDB
    let scylla = db::connect(&scylla_url).await?;
    db::migrate(&scylla).await?;

    // Connect to NATS
    let nats = async_nats::connect(&nats_url).await?;
    tracing::info!(%nats_url, "NATS connected");

    // Connect to Redis
    let redis = read_state::connect_redis().await;

    // S3 for attachments (optional)
    let s3 = if std::env::var("S3_ACCESS_KEY_ID").is_ok() {
        Some(Arc::new(
            matehub_common::storage::S3Storage::from_env("S3_BUCKET_CHAT_MEDIA").await,
        ))
    } else {
        tracing::warn!("S3_ACCESS_KEY_ID not set, attachments disabled");
        None
    };

    // Build services
    let data_service = Arc::new(data_service::DataService::new(scylla).await?);
    let fanout = Arc::new(fanout::FanoutService::new(nats.clone()));

    // Spawn transcode result consumer
    transcode::spawn_result_consumer(nats.clone(), data_service.clone(), fanout.clone());

    let sessions = session::SessionStore::new();

    // Background reaper for expired sessions (every 60s)
    let reaper_sessions = sessions.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            let reaped = reaper_sessions.reap_expired();
            if reaped > 0 {
                tracing::debug!(reaped, "expired sessions cleaned");
            }
        }
    });

    let state = AppState {
        data: data_service,
        fanout: fanout.clone(),
        redis,
        s3,
        sessions,
    };

    // Prometheus /metrics + HTTP-request instrumentation layer.
    let (metrics_layer, metrics_handle) =
        matehub_common::observability::metrics_layer_and_handle();

    // HTTP + WS
    let app = api::routes(state.clone())
        .merge(gateway::routes(state))
        .route(
            "/metrics",
            axum::routing::get({
                let h = metrics_handle.clone();
                move || async move { h.render() }
            }),
        )
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .layer(metrics_layer);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!(%port, %scylla_url, %nats_url, "matehub-chat started");

    axum::serve(listener, app).await?;
    Ok(())
}
