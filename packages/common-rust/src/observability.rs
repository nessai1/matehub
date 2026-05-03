//! Observability foundation: tracing init + Prometheus /metrics layer.
//!
//! # Tracing
//! See `init_tracing`. JSON output when `LOG_FORMAT=json`, pretty otherwise.
//!
//! # Metrics
//! `metrics_layer_and_handle` returns an axum layer (installs the global
//! metrics recorder on first call) plus a handle to render Prometheus output.
//! Each service mounts the handle under `GET /metrics` and applies the layer
//! to its router. HTTP request count / status / duration come for free via
//! `axum-prometheus`'s default metrics. Custom gauges / counters are emitted
//! through the `metrics` facade crate (e.g. `metrics::gauge!("foo", 42.0)`)
//! and end up in the same /metrics output.
//!
//! Two output modes, picked via `LOG_FORMAT`:
//!   - `json` — one JSON object per log line; span fields get flattened as
//!     top-level keys. Ready to be shipped by Filebeat to Elasticsearch
//!     without any extra parsing stage. Use in prod / staging.
//!   - anything else (including unset) — tracing-subscriber's default
//!     human-readable formatter. Use in local dev / tests.
//!
//! `RUST_LOG` overrides everything; otherwise the service-provided default
//! filter applies (e.g. `info,sqlx=warn` for Postgres-heavy services).

use tracing_subscriber::EnvFilter;

/// Initialize the global tracing subscriber. Call once at process start,
/// before any log macros fire. Subsequent calls panic (subscriber is
/// install-once).
pub fn init_tracing(default_filter: &str) {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));

    let format = std::env::var("LOG_FORMAT").unwrap_or_default();

    if format == "json" {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .json()
            // Hoist fields from the event itself to top-level so Kibana
            // shows them as first-class columns without a parse step.
            .flatten_event(true)
            // Include the current-span fields (call_id, hub_id, request_id,
            // etc). `with_span_list(false)` keeps only the innermost span —
            // we don't need the whole stack in each record.
            .with_current_span(true)
            .with_span_list(false)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    }
}

/// Build a Prometheus metrics layer + render handle for an axum service.
///
/// Usage in service main:
/// ```ignore
/// let (metrics_layer, metrics_handle) =
///     matehub_common::observability::metrics_layer_and_handle();
///
/// let app = Router::new()
///     .route("/metrics", axum::routing::get({
///         let h = metrics_handle.clone();
///         move || async move { h.render() }
///     }))
///     /* ...existing routes... */
///     .layer(metrics_layer);
/// ```
///
/// `/metrics` is excluded from being recorded itself (would otherwise
/// scrape-observe-scrape-observe forever). `/health` likewise.
pub fn metrics_layer_and_handle() -> (
    axum_prometheus::PrometheusMetricLayer<'static>,
    axum_prometheus::metrics_exporter_prometheus::PrometheusHandle,
) {
    axum_prometheus::PrometheusMetricLayerBuilder::new()
        .with_ignore_patterns(&["/metrics", "/health"])
        .with_default_metrics()
        .build_pair()
}
