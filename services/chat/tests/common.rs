use std::sync::Arc;
use tokio::net::TcpListener;

/// Spawn chat service on random port with real ScyllaDB + NATS.
/// Returns base URL.
pub async fn spawn_app() -> String {
    let scylla_url =
        std::env::var("SCYLLA_URL").unwrap_or_else(|_| "127.0.0.1:9042".into());
    let nats_url =
        std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());

    matehub_common::snowflake::init();

    let scylla = matehub_chat::db::connect(&scylla_url).await.unwrap();
    matehub_chat::db::migrate(&scylla).await.unwrap();

    let nats = async_nats::connect(&nats_url).await.unwrap();

    let data = Arc::new(
        matehub_chat::data_service::DataService::new(scylla)
            .await
            .unwrap(),
    );
    let fanout = Arc::new(matehub_chat::fanout::FanoutService::new(nats));

    let redis = matehub_chat::read_state::connect_redis().await;

    let state = matehub_chat::api::AppState {
        data,
        fanout: fanout.clone(),
        redis,
        s3: None, // S3 not needed for tests
        sessions: matehub_chat::session::SessionStore::new(),
    };

    let app = matehub_chat::api::routes(state.clone())
        .merge(matehub_chat::gateway::routes(state))
        .layer(tower_http::cors::CorsLayer::permissive());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://{addr}")
}

/// Create a test JWT (matches hub service's shared secret).
/// `sub` is derived deterministically from the username so separate test runs
/// see the same author_id and existing assertions stay stable.
pub fn test_jwt(username: &str, hub_id: i64) -> String {
    use jsonwebtoken::{EncodingKey, Header, encode};

    let secret = std::env::var("JWT_SECRET")
        .unwrap_or_else(|_| "matehub-dev-secret-change-in-prod".into());

    let now = chrono::Utc::now().timestamp();
    let sub = test_user_id(username);
    let claims = serde_json::json!({
        "sub": sub,
        "username": username,
        "user_type": "permanent",
        "hub_id": hub_id,
        "groups": [],
        "iat": now,
        "exp": now + 3600,
    });

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap()
}

/// Deterministic numeric user ID for a test username.
pub fn test_user_id(username: &str) -> i64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    username.hash(&mut h);
    (h.finish() & 0x7FFF_FFFF_FFFF_FFFF) as i64
}

#[allow(dead_code)]
pub fn ws_url(http_base: &str, path: &str) -> String {
    http_base.replace("http://", "ws://") + path
}
