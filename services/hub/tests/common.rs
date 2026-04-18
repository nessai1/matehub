use std::sync::Arc;

use tokio::net::TcpListener;

/// Spawn hub service on random port with real DB.
/// Returns base URL (e.g., "http://127.0.0.1:12345").
/// Requires PostgreSQL + Redis running locally.
///
/// Uses `matehub_test` database to avoid polluting dev data.
/// Truncates all tables + re-runs migrations + seed for clean state.
pub async fn spawn_app() -> String {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://matehub:matehub-dev@localhost:5432/matehub_test".into());

    let pool = matehub_hub::db::connect(&database_url).await.unwrap();

    // Run migrations (idempotent -- skips already applied)
    matehub_hub::db::migrate(&pool).await.unwrap();

    // Clean data tables in FK-safe order, then re-seed
    sqlx::raw_sql(
        "TRUNCATE temp_users, channel_permissions, member_groups, groups, channels, hub_members, refresh_tokens, hubs, users CASCADE;"
    )
    .execute(&pool)
    .await
    .ok();

    matehub_hub::db::seed::run_dev_seed(&pool).await.unwrap();

    let redis = matehub_hub::presence::connect_redis().await;

    let app = matehub_hub::api::routes(pool, None, redis, true)
        .layer(tower_http::cors::CorsLayer::permissive());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://{addr}")
}

/// Login as a dev user and return the JWT token.
pub async fn login(base: &str, username: &str) -> String {
    let client = reqwest::Client::new();
    let resp: serde_json::Value = client
        .post(format!("{base}/v1/auth/login"))
        .json(&serde_json::json!({
            "login": username,
            "password": "123123",
            "hub_id": "def00000-0000-0000-0000-000000000001"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    resp["access_token"].as_str().unwrap().to_string()
}

#[allow(dead_code)]
pub fn ws_url(http_base: &str, path: &str) -> String {
    http_base.replace("http://", "ws://") + path
}
