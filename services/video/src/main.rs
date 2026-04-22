mod api;
mod config;
mod sfu;
mod signaling;
mod state;

use std::net::UdpSocket;
use std::sync::Arc;

use anyhow::Result;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use config::Config;
use sfu::SfuEngine;
use state::AppState;

// SFU allocates on hot paths (RTP buffers, NACK cache, SRTP contexts).
// Default malloc is visibly slower than mimalloc under high-pps load.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> Result<()> {
    matehub_common::observability::init_tracing("info,str0m=warn");

    let config = Config::from_env();

    // Cross-thread SFU command channel. crossbeam so the sender can be called
    // synchronously from tokio WebSocket handlers (no .await) and the receiver
    // can be polled from a blocking OS thread (no async runtime needed).
    let (sfu_cmd_tx, sfu_cmd_rx) = crossbeam::channel::unbounded();

    // NATS connection for voice-occupancy events. Optional — if the box isn't
    // running NATS yet the video service still handles calls, just without
    // the sidebar roster push. Hub service degrades to polling.
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let nats = match async_nats::connect(&nats_url).await {
        Ok(c) => {
            tracing::info!(%nats_url, "NATS connected (voice occupancy)");
            Some(c)
        }
        Err(e) => {
            tracing::warn!(%nats_url, error = %e, "NATS connect failed — voice occupancy disabled");
            None
        }
    };

    let state = AppState::new(sfu_cmd_tx, nats);

    // Bind UDP socket for media — std (blocking) socket, not tokio::net.
    // Media path runs on its own OS thread; it uses SO_RCVTIMEO for the tick
    // cadence instead of sharing the tokio reactor with HTTP/WS.
    let udp_addr = format!("0.0.0.0:{}", config.udp_port);
    let udp_socket = UdpSocket::bind(&udp_addr)?;

    // Increase send/recv buffers to reduce packet drops under load.
    // Default ~200KB, set to 2MB. Covers burst of video keyframes + audio.
    let sock_ref = socket2::SockRef::from(&udp_socket);
    let _ = sock_ref.set_send_buffer_size(2 * 1024 * 1024);
    let _ = sock_ref.set_recv_buffer_size(2 * 1024 * 1024);
    let actual_send = sock_ref.send_buffer_size().unwrap_or(0);
    let actual_recv = sock_ref.recv_buffer_size().unwrap_or(0);

    let udp_socket = Arc::new(udp_socket);
    tracing::info!(addr = %udp_addr, send_buf = actual_send, recv_buf = actual_recv, "UDP media socket bound");

    // SFU engine on a dedicated OS thread, OUT of the tokio runtime.
    // Media forwarding latency no longer competes with signaling / HTTP work.
    let engine = SfuEngine::new(udp_socket, config.public_ips.clone(), sfu_cmd_rx);
    let media_thread = std::thread::Builder::new()
        .name("sfu-media".into())
        // str0m keeps a fair amount of per-Rtc state on the stack during
        // poll_output; default (2MB on macOS/Linux) is enough but pin it
        // explicitly so it doesn't depend on host defaults.
        .stack_size(2 * 1024 * 1024)
        .spawn(move || engine.run_blocking())?;
    // Hold the handle so the thread can outlive main's scope. If the thread
    // panics, `.join()` on shutdown would surface it; we currently run until
    // signalled so this is effectively "leak until process exit".
    std::mem::forget(media_thread);

    let (metrics_layer, metrics_handle) =
        matehub_common::observability::metrics_layer_and_handle();

    // HTTP + WebSocket server
    let app = api::routes(state)
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

    let http_addr = format!("0.0.0.0:{}", config.http_port);
    let listener = tokio::net::TcpListener::bind(&http_addr).await?;

    tracing::info!(
        http = %http_addr,
        udp = %udp_addr,
        public_ips = ?config.public_ips,
        "matehub-video started"
    );

    axum::serve(listener, app).await?;
    Ok(())
}
