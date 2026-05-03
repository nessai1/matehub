//! In-process CPU profiler endpoint. Compiled in only when the `profiling`
//! feature is enabled (see Cargo.toml).
//!
//! Usage:
//!   curl 'http://localhost:4000/debug/pprof/profile?seconds=30' > sfu.svg
//!
//! Then open sfu.svg in a browser. Wider boxes = more CPU time. Look for
//! `forward_media_now`, `mi_malloc`, `data.data.clone()` callsites — see
//! docs/video/profiling.md for the full interpretation guide.
//!
//! NOT a public production endpoint — never expose without a separate
//! authn gate (it returns useful runtime info AND consumes ~5% CPU during
//! the sample window).

use axum::{
    Router,
    extract::Query,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/debug/pprof/profile", get(profile_handler))
}

#[derive(Deserialize)]
struct ProfileQuery {
    /// Sample window length. Default 30s — long enough to catch a steady
    /// state, short enough that one curl invocation isn't disruptive.
    #[serde(default = "default_seconds")]
    seconds: u64,
    /// Sampling rate (Hz). Higher = more detail, more overhead. 99 is
    /// the standard prime to avoid lockstep with periodic timers.
    #[serde(default = "default_frequency")]
    frequency: i32,
}

fn default_seconds() -> u64 {
    30
}
fn default_frequency() -> i32 {
    99
}

async fn profile_handler(Query(q): Query<ProfileQuery>) -> Response {
    let seconds = q.seconds.clamp(1, 120);
    let guard = match pprof::ProfilerGuardBuilder::default()
        .frequency(q.frequency)
        // Skip libc/glibc internals so the flamegraph stays focused on
        // matehub frames. Standard pprof recipe.
        .blocklist(&["libc", "libgcc", "pthread", "vdso"])
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to start profiler: {e}"),
            )
                .into_response();
        }
    };

    tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;

    let report = match guard.report().build() {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to build profile report: {e}"),
            )
                .into_response();
        }
    };

    let mut svg_buf: Vec<u8> = Vec::new();
    if let Err(e) = report.flamegraph(&mut svg_buf) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to render flamegraph: {e}"),
        )
            .into_response();
    }

    ([(header::CONTENT_TYPE, "image/svg+xml")], svg_buf).into_response()
}
