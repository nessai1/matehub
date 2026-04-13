mod api;
mod db;
mod models;

use anyhow::Result;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

#[tokio::main]
async fn main() -> Result<()> {
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
        .unwrap_or(true); // default true for now

    let pool = db::connect(&database_url).await?;
    db::migrate(&pool).await?;

    if dev_mode {
        db::seed::run_dev_seed(&pool).await?;
    }

    let app = api::routes(pool, dev_mode)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .route("/health", axum::routing::get(|| async { "ok" }));

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!(%port, %dev_mode, "matehub-hub started");

    axum::serve(listener, app).await?;
    Ok(())
}
