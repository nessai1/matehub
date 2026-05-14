// Integration tests for the first-run setup wizard.
//
// Two pieces under test, both regressions from MAT-10 / MAT-13 on alpha:
//
// 1. `POST /v1/setup/admin` must grant the everyone group MEMBER_CHANNEL
//    bits on the bootstrap #general channel. Without that row,
//    `chat::access::check_uncached` walks the `channel_permissions JOIN
//    member_groups` shape, finds no match for non-admin members, falls
//    through to 0 effective bits, and answers 403 on /messages. Admin
//    bypasses via `is_admin = true`, which is why this stayed hidden in
//    dev (dev seed adds the row out-of-band) and in CI (same).
//
// 2. `GET /v1/setup/status` must surface the hub identity once the wizard
//    has finished, so the SPA's login screen can submit `hub_id` for the
//    actual deployment instead of a hard-coded DEV_HUB_ID="1". Without
//    that, every box install where the wizard assigned a Snowflake
//    hub_id returns 403 on every login attempt.
//
// The test spawns a fresh app instance per case so the bootstrap-once
// (`pg_advisory_xact_lock` + `hub_members LIMIT 1`) guard doesn't trip
// across cases.

use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::net::TcpListener;

use matehub_hub::db::seed::DEV_HUB_ID;

/// Spawn the hub service against a freshly-truncated DB **without** the
/// dev seed. Tests in this file need to exercise the actual `setup_admin`
/// handler (and its `INSERT INTO channel_permissions` step) against an
/// empty hub_members table; the shared `common::spawn_app` always
/// re-seeds, which would short-circuit the wizard with 409.
async fn spawn_unseeded_app() -> String {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://matehub:matehub-dev@localhost:5432/matehub_test".into());

    matehub_common::snowflake::init();

    let pool = matehub_hub::db::connect(&database_url).await.unwrap();
    matehub_hub::db::migrate(&pool).await.unwrap();

    sqlx::raw_sql(
        "TRUNCATE invite_links, temp_users, channel_permissions, member_groups, groups, channels, hub_members, refresh_tokens, hubs, users CASCADE;"
    )
    .execute(&pool)
    .await
    .ok();

    let redis = matehub_hub::presence::connect_redis().await;
    let (events_tx, _) = tokio::sync::broadcast::channel(16);

    let sso_state = matehub_hub::api::sso::SsoState {
        pool: pool.clone(),
        general_url: None,
        hub_id: DEV_HUB_ID,
    };

    let app = matehub_hub::api::routes(pool, None, redis, events_tx, true, sso_state)
        .layer(tower_http::cors::CorsLayer::permissive());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://{addr}")
}

/// Open a direct connection to the test DB. Used to assert on rows the
/// REST API doesn't expose (channel_permissions, etc.) without inflating
/// the public surface area just for testing.
async fn raw_pool() -> sqlx::PgPool {
    let url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://matehub:matehub-dev@localhost:5432/matehub_test".into());
    sqlx::PgPool::connect(&url).await.unwrap()
}

async fn run_setup(base: &str) -> Value {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/v1/setup/admin"))
        .json(&json!({
            "username": "boxadmin",
            "password": "secret123",
            "display_name": "Box Admin",
            "email": "admin@box.test",
            "hub_name": "Box Hub",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "setup_admin should succeed on unseeded DB"
    );
    resp.json().await.unwrap()
}

// ── MAT-13 regression ────────────────────────────────────────────

#[tokio::test]
#[serial_test::serial(setup_db)]
async fn setup_grants_everyone_member_bits_on_general_channel() {
    let base = spawn_unseeded_app().await;
    let body = run_setup(&base).await;
    let hub_id: i64 = body["hub_id"].as_str().unwrap().parse().unwrap();

    let pool = raw_pool().await;

    // Locate the #general channel created by setup.
    let channel_id: i64 = sqlx::query_scalar(
        "SELECT id FROM channels WHERE hub_id = $1 AND name = 'general' AND type = 'text'",
    )
    .bind(hub_id)
    .fetch_one(&pool)
    .await
    .expect("setup must create #general");

    // Locate the everyone group.
    let everyone_id: i64 =
        sqlx::query_scalar("SELECT id FROM groups WHERE hub_id = $1 AND is_default = true LIMIT 1")
            .bind(hub_id)
            .fetch_one(&pool)
            .await
            .expect("setup must create the everyone group");

    // The fix: this row must exist with MEMBER_CHANNEL bits (31), so
    // non-admin members get READ + WRITE + CONNECT + SPEAK + VIDEO and
    // chat::access::check answers true on Read / Write.
    let perms: Option<(i32, i32)> = sqlx::query_as(
        "SELECT allow_bits, deny_bits FROM channel_permissions
         WHERE channel_id = $1 AND group_id = $2",
    )
    .bind(channel_id)
    .bind(everyone_id)
    .fetch_optional(&pool)
    .await
    .unwrap();

    let (allow, deny) = perms.expect("everyone group must have a permissions row on #general");
    assert_eq!(
        allow,
        matehub_common::perms::bits::MEMBER_CHANNEL,
        "everyone allow_bits must be MEMBER_CHANNEL preset (31)"
    );
    assert_eq!(deny, 0, "everyone deny_bits must be empty");
}

// ── MAT-10 regression ────────────────────────────────────────────

#[tokio::test]
#[serial_test::serial(setup_db)]
async fn status_reports_hub_id_once_setup_is_done() {
    let base = spawn_unseeded_app().await;
    let body = run_setup(&base).await;
    let setup_hub_id = body["hub_id"].as_str().unwrap().to_string();

    let resp = reqwest::Client::new()
        .get(format!("{base}/v1/setup/status"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let status: Value = resp.json().await.unwrap();

    assert_eq!(status["needs_setup"], false);
    assert_eq!(
        status["hub_id"]
            .as_str()
            .expect("hub_id must be present and a string"),
        setup_hub_id,
        "status hub_id must match the one the wizard returned"
    );
    assert_eq!(status["hub_slug"], "default");
    assert_eq!(status["hub_name"], "Box Hub");
}

#[tokio::test]
#[serial_test::serial(setup_db)]
async fn status_omits_hub_id_before_setup() {
    let base = spawn_unseeded_app().await;

    let resp = reqwest::Client::new()
        .get(format!("{base}/v1/setup/status"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let status: Value = resp.json().await.unwrap();

    assert_eq!(status["needs_setup"], true);
    // hub_id / hub_slug / hub_name are skipped from the response when
    // there's no hub yet (#[serde(skip_serializing_if = "Option::is_none")]).
    assert!(
        status.get("hub_id").map(Value::is_null).unwrap_or(true),
        "hub_id must be absent before setup, got: {status:?}"
    );
}

// ── End-to-end: redeem invite → fresh login through /auth/login ──
//
// MAT-10's user-visible failure was "newbie redeems an invite link, then
// can't log back in with the same credentials". The frontend bug
// (hard-coded hub_id) is one half; this test pins the backend half: if
// the SPA sends the correct hub_id, the login MUST succeed for the
// invite-link user. If this ever fails, the regression is in the
// auth/login + hub_members read path, not the FE.

#[tokio::test]
#[serial_test::serial(setup_db)]
async fn invite_link_user_can_login_after_redeem() {
    let base = spawn_unseeded_app().await;
    let setup_body = run_setup(&base).await;
    let admin_token = setup_body["access_token"].as_str().unwrap().to_string();
    let hub_id_str = setup_body["hub_id"].as_str().unwrap().to_string();

    let client = reqwest::Client::new();

    // Admin creates an invite-link (no group_id → everyone-only).
    let create_resp = client
        .post(format!("{base}/v1/hubs/{hub_id_str}/invite-links"))
        .bearer_auth(&admin_token)
        .json(&json!({
            "expires_at": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(create_resp.status(), StatusCode::CREATED);
    let invite: Value = create_resp.json().await.unwrap();
    let invite_token = invite["token"].as_str().unwrap();

    // Visitor redeems it.
    let redeem_resp = client
        .post(format!("{base}/v1/invite-links/{invite_token}/redeem"))
        .json(&json!({
            "username": "newbie01",
            "password": "secret123",
            "display_name": "Newbie",
            "email": "newbie@box.test",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(redeem_resp.status(), 200);

    // The actual MAT-10 scenario: a fresh /auth/login from the same
    // creds, with the *correct* hub_id (i.e. the one /v1/setup/status
    // would surface). This must succeed.
    let login_resp = client
        .post(format!("{base}/v1/auth/login"))
        .json(&json!({
            "login": "newbie01",
            "password": "secret123",
            "hub_id": hub_id_str,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        login_resp.status(),
        200,
        "invite-link user must be able to re-login with the deployment's real hub_id"
    );
    let login_body: Value = login_resp.json().await.unwrap();
    assert_eq!(login_body["username"], "newbie01");
    assert_eq!(login_body["hub_id"], hub_id_str);
}
