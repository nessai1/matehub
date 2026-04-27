use std::sync::Arc;
use tokio::net::TcpListener;

/// Spawn chat service on random port with real ScyllaDB + NATS + hub Postgres.
/// Returns base URL.
///
/// Postgres setup mirrors the hub test harness: truncate, re-migrate, re-seed.
/// We need a live PG so access::check resolves correctly — without it every
/// chat endpoint would 403.
pub async fn spawn_app() -> String {
    let scylla_url =
        std::env::var("SCYLLA_URL").unwrap_or_else(|_| "127.0.0.1:9042".into());
    let nats_url =
        std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let pg_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://matehub:matehub-dev@localhost:5432/matehub_test".into());

    matehub_common::snowflake::init();

    let scylla = matehub_chat::db::connect(&scylla_url).await.unwrap();
    matehub_chat::db::migrate(&scylla).await.unwrap();

    let nats = async_nats::connect(&nats_url).await.unwrap();

    // Hub PG: connect, run migrations, truncate, reseed. Same pattern as
    // services/hub/tests/common.rs.
    let pg = matehub_hub::db::connect(&pg_url).await.unwrap();
    matehub_hub::db::migrate(&pg).await.unwrap();
    sqlx::raw_sql(
        "TRUNCATE temp_users, channel_permissions, dm_participants, member_groups, groups, channels, hub_members, refresh_tokens, hubs, users CASCADE;"
    )
    .execute(&pg)
    .await
    .ok();
    matehub_hub::db::seed::run_dev_seed(&pg).await.unwrap();

    // Legacy chat tests use a separate hub (1001) and arbitrary channel ids
    // (2001, 3001, …). Stamp them into PG so access::check finds the right
    // group → channel_permissions rows and lets the tests through.
    seed_legacy_test_hub(&pg).await;

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
        pg: Some(pg),
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

/// Stamp the legacy chat-test data into hub PG: hub 1001 with alice/bob/charlie
/// as members of an "everyone" group with full READ/WRITE on every channel id
/// the tests bake in. Idempotent via ON CONFLICT DO NOTHING.
async fn seed_legacy_test_hub(pg: &sqlx::PgPool) {
    seed_legacy_one(pg, 1001, 1_900_001, &[2001, 3001, 4001, 5001, 6001]).await;
    seed_legacy_one(pg, 7001, 1_900_002, &[8001]).await;
}

async fn seed_legacy_one(
    pg: &sqlx::PgPool,
    legacy_hub: i64,
    legacy_group: i64,
    channels: &[i64],
) {
    use matehub_hub::db::seed::{DEV_USER_ALICE, DEV_USER_BOB, DEV_USER_CHARLIE};
    const RW_BITS: i32 = 1 | 2; // READ | WRITE

    let slug = format!("legacy-test-{legacy_hub}");
    sqlx::query(
        "INSERT INTO hubs (id, name, slug, plan, creator_id) VALUES ($1, $2, $2, 'free', $3)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(legacy_hub)
    .bind(&slug)
    .bind(DEV_USER_ALICE)
    .execute(pg)
    .await
    .unwrap();

    for uid in &[DEV_USER_ALICE, DEV_USER_BOB, DEV_USER_CHARLIE] {
        sqlx::query(
            "INSERT INTO hub_members (hub_id, user_id, role) VALUES ($1, $2, 'member')
             ON CONFLICT DO NOTHING",
        )
        .bind(legacy_hub)
        .bind(uid)
        .execute(pg)
        .await
        .unwrap();
    }

    sqlx::query(
        "INSERT INTO groups (id, hub_id, name, color, position, is_default, hub_permissions)
         VALUES ($1, $2, 'everyone', '#888', 100, true, 0)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(legacy_group)
    .bind(legacy_hub)
    .execute(pg)
    .await
    .unwrap();

    for uid in &[DEV_USER_ALICE, DEV_USER_BOB, DEV_USER_CHARLIE] {
        sqlx::query(
            "INSERT INTO member_groups (hub_id, user_id, group_id) VALUES ($1, $2, $3)
             ON CONFLICT DO NOTHING",
        )
        .bind(legacy_hub)
        .bind(uid)
        .bind(legacy_group)
        .execute(pg)
        .await
        .unwrap();
    }

    for ch in channels {
        sqlx::query(
            "INSERT INTO channels (id, hub_id, name, type, position) VALUES ($1, $2, $3, 'text', 0)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(ch)
        .bind(legacy_hub)
        .bind(format!("test-{ch}"))
        .execute(pg)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO channel_permissions (channel_id, group_id, allow_bits, deny_bits) VALUES ($1, $2, $3, 0)
             ON CONFLICT DO NOTHING",
        )
        .bind(ch)
        .bind(legacy_group)
        .bind(RW_BITS)
        .execute(pg)
        .await
        .unwrap();
    }
}

/// Map test usernames onto the dev seed snowflakes. Tests reuse the DEV_HUB_ID
/// hub and its three seeded users (alice/bob/charlie) so access::check has a
/// real ACL graph to walk.
pub fn test_user_id(username: &str) -> i64 {
    match username {
        "alice" => matehub_hub::db::seed::DEV_USER_ALICE,
        "bob" => matehub_hub::db::seed::DEV_USER_BOB,
        "charlie" => matehub_hub::db::seed::DEV_USER_CHARLIE,
        // Unknown user → fall back to a hash. Won't pass access::check
        // because they're not in hub_members, but the test still gets a
        // stable id for assertions.
        other => {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut h = DefaultHasher::new();
            other.hash(&mut h);
            (h.finish() & 0x7FFF_FFFF_FFFF_FFFF) as i64
        }
    }
}

/// Look up a seeded channel id by name. Tests rely on `general`, `random`,
/// `voice-test`, `stage-test` from `run_dev_seed`.
#[allow(dead_code)]
pub async fn seed_channel_id(pg: &sqlx::PgPool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT id FROM channels WHERE hub_id = $1 AND name = $2",
    )
    .bind(matehub_hub::db::seed::DEV_HUB_ID)
    .bind(name)
    .fetch_one(pg)
    .await
    .unwrap_or_else(|e| panic!("seed channel '{name}' not found: {e}"))
}

#[allow(dead_code)]
pub fn ws_url(http_base: &str, path: &str) -> String {
    http_base.replace("http://", "ws://") + path
}
