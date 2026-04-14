mod api;
mod auth;
mod db;
mod models;
mod storage;

use std::sync::Arc;

use anyhow::Result;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env -- try workspace root, then current dir, then search upward
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

    // S3 storage (optional -- skip if no credentials)
    let s3 = if std::env::var("S3_ACCESS_KEY_ID").is_ok() {
        Some(Arc::new(storage::S3Storage::from_env().await))
    } else {
        tracing::warn!("S3_ACCESS_KEY_ID not set, avatar uploads disabled");
        None
    };

    let app = api::routes(pool, s3, dev_mode)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .route("/health", axum::routing::get(|| async { "ok" }));

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!(%port, %dev_mode, "matehub-hub started");

    axum::serve(listener, app).await?;
    Ok(())
}
