mod api;
mod config;
mod sfu;
mod signaling;
mod state;

use std::sync::Arc;

use anyhow::Result;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use config::Config;
use sfu::SfuEngine;
use state::AppState;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,str0m=warn".into()),
        )
        .init();

    let config = Config::from_env();

    // SFU command channel (WS handlers -> SFU engine)
    let (sfu_cmd_tx, sfu_cmd_rx) = mpsc::unbounded_channel();

    let state = AppState::new(sfu_cmd_tx);

    // Bind UDP socket for media
    let udp_addr = format!("0.0.0.0:{}", config.udp_port);
    let udp_socket = Arc::new(UdpSocket::bind(&udp_addr).await?);
    tracing::info!(addr = %udp_addr, "UDP media socket bound");

    // Start SFU engine
    let engine = SfuEngine::new(udp_socket, config.public_ip, sfu_cmd_rx);
    tokio::spawn(async move {
        engine.run().await;
    });

    // HTTP + WebSocket server
    let app = api::routes(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let http_addr = format!("0.0.0.0:{}", config.http_port);
    let listener = tokio::net::TcpListener::bind(&http_addr).await?;

    tracing::info!(
        http = %http_addr,
        udp = %udp_addr,
        public_ip = %config.public_ip,
        "matehub-video started"
    );

    axum::serve(listener, app).await?;
    Ok(())
}
