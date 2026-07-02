mod api;
mod config;
mod debug_capture;
mod sfu;
mod signaling;
mod state;

use std::net::UdpSocket;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use config::Config;
use debug_capture::DebugCapture;
use sfu::SfuPool;
use state::AppState;

// SFU allocates on hot paths (RTP buffers, NACK cache, SRTP contexts).
// Default malloc is visibly slower than mimalloc under high-pps load.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> Result<()> {
    // ROOM_DEBUG=1 turns on per-call capture: a debug-level JSON tee of server
    // logs plus an endpoint for client debug bundles, both keyed by call id.
    // Off by default — zero overhead and the tracing setup is unchanged.
    let debug_capture = init_observability();

    let config = Config::from_env();

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

    // Bind UDP socket for media — std (blocking) socket, not tokio::net.
    // Media path runs on its own OS threads; uses SO_RCVTIMEO for the tick
    // cadence instead of sharing the tokio reactor with HTTP/WS.
    let udp_addr = format!("0.0.0.0:{}", config.udp_port);
    let udp_socket = UdpSocket::bind(&udp_addr)?;

    // Bigger buffers to absorb burst (video keyframes + audio fan-out).
    // Default ~200KB, set 8MB — large calls fan out tens of MB/s and 2MB
    // turned into the EAGAIN trigger we count via udp_send_drops_total.
    let sock_ref = socket2::SockRef::from(&udp_socket);
    let _ = sock_ref.set_send_buffer_size(8 * 1024 * 1024);
    let _ = sock_ref.set_recv_buffer_size(8 * 1024 * 1024);
    let actual_send = sock_ref.send_buffer_size().unwrap_or(0);
    let actual_recv = sock_ref.recv_buffer_size().unwrap_or(0);

    let udp_socket = Arc::new(udp_socket);
    tracing::info!(addr = %udp_addr, send_buf = actual_send, recv_buf = actual_recv, "UDP media socket bound");

    // Sharded SFU: N media threads (one per shard) + one UDP dispatcher.
    // The dispatcher reads all incoming UDP, parses STUN ufrag for new
    // sources, and forwards pre-resolved packets to the right shard via
    // crossbeam. Sessions live entirely on one shard each (consistent
    // hash on session_id) — no cross-shard fan-out on the hot path.
    let num_shards = std::env::var("MEDIA_SHARDS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(1);
    let cmd_buffer = std::env::var("SFU_CMD_BUFFER")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(8192);
    let sfu_pool = SfuPool::spawn(
        num_shards,
        Arc::clone(&udp_socket),
        config.public_ips.clone(),
        std::time::Duration::from_secs(config.zombie_timeout_secs),
        cmd_buffer,
    );
    tracing::info!(shards = num_shards, cmd_buffer, "SFU pool started");

    let state = AppState::new(sfu_pool, nats, debug_capture);

    let (metrics_layer, metrics_handle) = matehub_common::observability::metrics_layer_and_handle();

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

/// Install tracing and, when `ROOM_DEBUG=1`, the per-call capture pipeline.
/// Returns the capture handle (for the HTTP bundle endpoint) or `None`.
///
/// Env:
/// * `ROOM_DEBUG=1` — enable capture.
/// * `ROOM_DEBUG_DIR` — capture root (default `room-debug`).
/// * `ROOM_DEBUG_FILTER` — capture-file log filter (default
///   `matehub_video=debug,str0m=warn`).
fn init_observability() -> Option<Arc<DebugCapture>> {
    const STDOUT_FILTER: &str = "info,str0m=warn";

    if std::env::var("ROOM_DEBUG").as_deref() != Ok("1") {
        matehub_common::observability::init_tracing(STDOUT_FILTER);
        return None;
    }

    let dir = std::env::var("ROOM_DEBUG_DIR").unwrap_or_else(|_| "room-debug".into());
    // Run id keeps each process's server log in its own file so restarts don't
    // interleave. Time-based; chrono is already a dependency.
    let run_id = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string();

    match DebugCapture::init(PathBuf::from(&dir), &run_id) {
        Ok((handle, writer)) => {
            let filter = std::env::var("ROOM_DEBUG_FILTER")
                .unwrap_or_else(|_| "matehub_video=debug,str0m=warn".into());
            matehub_common::observability::init_tracing_with_capture(
                STDOUT_FILTER,
                Some(matehub_common::observability::CaptureLayer { filter, writer }),
            );
            tracing::warn!(
                dir = %handle.dir().display(),
                run_id,
                "ROOM_DEBUG capture ENABLED — per-call server+client bundles will be written"
            );
            Some(handle)
        }
        Err(e) => {
            // Can't open the capture dir — fall back to normal logging rather
            // than refuse to start. Loud about it so it's not a silent miss.
            matehub_common::observability::init_tracing(STDOUT_FILTER);
            tracing::error!(dir = %dir, error = %e, "ROOM_DEBUG requested but capture init failed — running without capture");
            None
        }
    }
}
