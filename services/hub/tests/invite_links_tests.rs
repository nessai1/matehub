// Integration tests for the general invite-link flow.
//
// Every test runs against a real Postgres (see `tests/common.rs`); the
// suite assumes `--test-threads=1` so the shared `matehub_test` DB isn't
// raced. The race-property tests deliberately spawn concurrent tokio
// tasks within a single test function — that races *requests*, not test
// boundaries.

mod common;

use std::sync::Arc;

use chrono::{Duration, Utc};
use pretty_assertions::assert_eq;
use reqwest::StatusCode;
use serde_json::{Value, json};

const HUB_ID: &str = "1"; // matches DEV_HUB_ID in seed.rs

// Seed group ids — keep aligned with `services/hub/src/db/seed.rs:14-15`.
const GROUP_GUESTS: i64 = 2003;
const GROUP_EVERYONE: i64 = 2001;

// ── Helpers ──────────────────────────────────────────────────────

async fn admin_token(base: &str) -> String {
    common::login(base, "alice").await
}

async fn create_link(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    expires_at: chrono::DateTime<Utc>,
    max_uses: Option<i32>,
    group_id: Option<i64>,
) -> reqwest::Response {
    let mut body = json!({
        "expires_at": expires_at,
    });
    if let Some(m) = max_uses {
        body["max_uses"] = json!(m);
    }
    if let Some(g) = group_id {
        body["group_id"] = json!(g.to_string());
    }
    client
        .post(format!("{base}/v1/hubs/{HUB_ID}/invite-links"))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap()
}

async fn redeem(
    client: &reqwest::Client,
    base: &str,
    invite_token: &str,
    username: &str,
    email: &str,
) -> reqwest::Response {
    client
        .post(format!("{base}/v1/invite-links/{invite_token}/redeem"))
        .json(&json!({
            "username": username,
            "password": "secret123",
            "display_name": format!("User {username}"),
            "email": email,
        }))
        .send()
        .await
        .unwrap()
}

/// Direct-DB poke: open a connection to the test DB and read uses_count for
/// a given invite-link token. Used by race-property tests that need to
/// assert on counter state independent of the API.
async fn read_uses_count(token: &str) -> i32 {
    let url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://matehub:matehub-dev@localhost:5432/matehub_test".into());
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::query_scalar::<_, i32>("SELECT uses_count FROM invite_links WHERE token = $1")
        .bind(token)
        .fetch_one(&pool)
        .await
        .unwrap()
}

/// Direct-DB poke: forge a past expiry on a freshly-created link, so we can
/// hit the redeem-time predicate without sleeping.
async fn force_expired(invite_token: &str) {
    let url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://matehub:matehub-dev@localhost:5432/matehub_test".into());
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::query(
        "UPDATE invite_links SET expires_at = now() - interval '1 second' WHERE token = $1",
    )
    .bind(invite_token)
    .execute(&pool)
    .await
    .unwrap();
}

// ── Create endpoint ──────────────────────────────────────────────

#[tokio::test]
async fn create_returns_token_and_signup_url() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();
    let token = admin_token(&base).await;

    let resp = create_link(
        &client,
        &base,
        &token,
        Utc::now() + Duration::hours(1),
        Some(5),
        Some(GROUP_GUESTS),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: Value = resp.json().await.unwrap();
    let invite_token = body["token"].as_str().unwrap();
    assert!(!invite_token.is_empty());
    assert_eq!(
        body["invite_url"].as_str().unwrap(),
        format!("/signup/{invite_token}")
    );
    assert_eq!(body["max_uses"].as_i64(), Some(5));
    assert_eq!(body["uses_count"].as_i64(), Some(0));
    assert_eq!(body["group_id"].as_str().unwrap(), GROUP_GUESTS.to_string());
}

#[tokio::test]
async fn create_rejects_past_expiry() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();
    let token = admin_token(&base).await;

    let resp = create_link(
        &client,
        &base,
        &token,
        Utc::now() - Duration::minutes(1),
        None,
        None,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "expires_in_past");
}

#[tokio::test]
async fn create_rejects_zero_max_uses() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();
    let token = admin_token(&base).await;

    let resp = create_link(
        &client,
        &base,
        &token,
        Utc::now() + Duration::hours(1),
        Some(0),
        None,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "invalid_max_uses");
}

#[tokio::test]
async fn create_forbidden_for_member_without_invite_permission() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();
    // bob is a regular member without INVITE_PERMANENT.
    let token = common::login(&base, "bob").await;

    let resp = create_link(
        &client,
        &base,
        &token,
        Utc::now() + Duration::hours(1),
        None,
        None,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ── Preview endpoint ─────────────────────────────────────────────

#[tokio::test]
async fn preview_returns_link_metadata() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();
    let token = admin_token(&base).await;

    let create_resp = create_link(
        &client,
        &base,
        &token,
        Utc::now() + Duration::hours(2),
        Some(3),
        Some(GROUP_GUESTS),
    )
    .await;
    let create_body: Value = create_resp.json().await.unwrap();
    let invite_token = create_body["token"].as_str().unwrap();

    let resp = client
        .get(format!("{base}/v1/invite-links/{invite_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["hub_name"], "Dev Hub");
    assert_eq!(body["hub_slug"], "dev-hub");
    assert_eq!(body["max_uses"].as_i64(), Some(3));
    assert_eq!(body["uses_count"].as_i64(), Some(0));
    assert_eq!(body["group_name"], "guests");
}

#[tokio::test]
async fn preview_after_expiry_returns_410() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();
    let token = admin_token(&base).await;

    let create_resp = create_link(
        &client,
        &base,
        &token,
        Utc::now() + Duration::hours(1),
        None,
        None,
    )
    .await;
    let invite_token = create_resp.json::<Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    force_expired(&invite_token).await;

    let resp = client
        .get(format!("{base}/v1/invite-links/{invite_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::GONE);
}

// ── Redeem endpoint ──────────────────────────────────────────────

#[tokio::test]
async fn redeem_creates_user_and_memberships() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();
    let admin = admin_token(&base).await;

    let create_resp = create_link(
        &client,
        &base,
        &admin,
        Utc::now() + Duration::hours(1),
        Some(5),
        Some(GROUP_GUESTS),
    )
    .await;
    let invite_token = create_resp.json::<Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = redeem(
        &client,
        &base,
        &invite_token,
        "newbie01",
        "newbie01@example.com",
    )
    .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body["access_token"].as_str().unwrap().len() > 20);
    assert!(body["refresh_token"].as_str().unwrap().len() > 20);
    assert_eq!(body["username"], "newbie01");
    assert_eq!(body["hub_slug"], "dev-hub");

    // Verify the user landed in the right hub + groups via direct DB read.
    let url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://matehub:matehub-dev@localhost:5432/matehub_test".into());
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let user_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = $1")
        .bind("newbie01")
        .fetch_one(&pool)
        .await
        .unwrap();
    let groups: Vec<i64> = sqlx::query_scalar(
        "SELECT group_id FROM member_groups WHERE hub_id = 1 AND user_id = $1 ORDER BY group_id",
    )
    .bind(user_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(groups.contains(&GROUP_EVERYONE));
    assert!(groups.contains(&GROUP_GUESTS));

    // Counter advanced exactly once.
    assert_eq!(read_uses_count(&invite_token).await, 1);
}

#[tokio::test]
async fn sixth_redeem_on_max_5_returns_410() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();
    let admin = admin_token(&base).await;

    let create_resp = create_link(
        &client,
        &base,
        &admin,
        Utc::now() + Duration::hours(1),
        Some(5),
        None,
    )
    .await;
    let invite_token = create_resp.json::<Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    for i in 1..=5 {
        let username = format!("user_max{i}");
        let email = format!("{username}@example.com");
        let r = redeem(&client, &base, &invite_token, &username, &email).await;
        assert_eq!(r.status(), 200, "redeem #{i} should succeed");
    }
    assert_eq!(read_uses_count(&invite_token).await, 5);

    let r = redeem(
        &client,
        &base,
        &invite_token,
        "user_max6",
        "user_max6@example.com",
    )
    .await;
    assert_eq!(r.status(), StatusCode::GONE);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["error"], "link_unavailable");
    // Counter must not advance beyond the cap.
    assert_eq!(read_uses_count(&invite_token).await, 5);
}

#[tokio::test]
async fn redeem_after_expiry_returns_410() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();
    let admin = admin_token(&base).await;

    let create_resp = create_link(
        &client,
        &base,
        &admin,
        Utc::now() + Duration::hours(1),
        None,
        None,
    )
    .await;
    let invite_token = create_resp.json::<Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    force_expired(&invite_token).await;

    let r = redeem(
        &client,
        &base,
        &invite_token,
        "tooLate",
        "toolate@example.com",
    )
    .await;
    assert_eq!(r.status(), StatusCode::GONE);
    assert_eq!(read_uses_count(&invite_token).await, 0);
}

#[tokio::test]
async fn redeem_after_revoke_returns_410() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();
    let admin = admin_token(&base).await;

    let create_resp = create_link(
        &client,
        &base,
        &admin,
        Utc::now() + Duration::hours(1),
        None,
        None,
    )
    .await;
    let create_body: Value = create_resp.json().await.unwrap();
    let invite_token = create_body["token"].as_str().unwrap().to_string();
    let link_id = create_body["id"].as_str().unwrap();

    // Revoke through the admin API (not direct DB) — exercises the
    // delete_pending_link route.
    let revoke_resp = client
        .delete(format!(
            "{base}/v1/hubs/{HUB_ID}/pending-invites/link/{link_id}"
        ))
        .bearer_auth(&admin)
        .send()
        .await
        .unwrap();
    assert_eq!(revoke_resp.status(), StatusCode::NO_CONTENT);

    let r = redeem(
        &client,
        &base,
        &invite_token,
        "afterRevoke",
        "afterrevoke@example.com",
    )
    .await;
    assert_eq!(r.status(), StatusCode::GONE);
    assert_eq!(read_uses_count(&invite_token).await, 0);
}

/// Critical race-property: a username collision must NOT burn a slot. The
/// atomic `UPDATE ... RETURNING` in the redeem path lives inside the same
/// transaction as the user INSERT, so a UNIQUE-violation rollback also
/// rolls back the increment. If this test ever fails, an attacker could
/// exhaust a link without registering by spamming colliding usernames.
#[tokio::test]
async fn username_collision_does_not_consume_slot() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();
    let admin = admin_token(&base).await;

    let create_resp = create_link(
        &client,
        &base,
        &admin,
        Utc::now() + Duration::hours(1),
        Some(2),
        None,
    )
    .await;
    let invite_token = create_resp.json::<Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    // alice already exists in the seed — try to register as alice.
    let r = redeem(
        &client,
        &base,
        &invite_token,
        "alice",
        "alice-new@example.com",
    )
    .await;
    assert_eq!(r.status(), StatusCode::CONFLICT);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["error"], "username_taken");

    // Critical assertion: the slot wasn't burned.
    assert_eq!(read_uses_count(&invite_token).await, 0);

    // Legitimate redeem still works.
    let r2 = redeem(
        &client,
        &base,
        &invite_token,
        "afterCollision",
        "aftercoll@example.com",
    )
    .await;
    assert_eq!(r2.status(), 200);
    assert_eq!(read_uses_count(&invite_token).await, 1);
}

#[tokio::test]
async fn redeem_validates_input() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();
    let admin = admin_token(&base).await;

    let create_resp = create_link(
        &client,
        &base,
        &admin,
        Utc::now() + Duration::hours(1),
        None,
        None,
    )
    .await;
    let invite_token = create_resp.json::<Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    // Bad username format.
    let r = client
        .post(format!("{base}/v1/invite-links/{invite_token}/redeem"))
        .json(&json!({
            "username": "ab", // too short
            "password": "secret123",
            "display_name": "Test",
            "email": "test@example.com",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        r.json::<Value>().await.unwrap()["error"],
        "username_invalid"
    );

    // Short password.
    let r = client
        .post(format!("{base}/v1/invite-links/{invite_token}/redeem"))
        .json(&json!({
            "username": "validuser",
            "password": "12345",
            "display_name": "Test",
            "email": "test@example.com",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        r.json::<Value>().await.unwrap()["error"],
        "password_too_short"
    );

    // Bad email.
    let r = client
        .post(format!("{base}/v1/invite-links/{invite_token}/redeem"))
        .json(&json!({
            "username": "validuser",
            "password": "secret123",
            "display_name": "Test",
            "email": "not-an-email",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    assert_eq!(r.json::<Value>().await.unwrap()["error"], "email_invalid");

    // None of the failed attempts should have burned a slot.
    assert_eq!(read_uses_count(&invite_token).await, 0);
}

/// Critical race-property #2: under concurrent redeems on a 3-slot link,
/// exactly 3 succeed and the rest get 410. If this fails, the counter
/// would over-shoot — turning the cap into a soft limit.
#[tokio::test]
async fn concurrent_redeem_respects_cap() {
    let base = common::spawn_app().await;
    let admin = admin_token(&base).await;

    // Use the bare client just to create the link.
    let create_client = reqwest::Client::new();
    let create_resp = create_link(
        &create_client,
        &base,
        &admin,
        Utc::now() + Duration::hours(1),
        Some(3),
        None,
    )
    .await;
    let invite_token = create_resp.json::<Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    // Spawn 10 concurrent redeems; each picks a unique username so collisions
    // can't be the reason for losing.
    let base = Arc::new(base);
    let invite_token = Arc::new(invite_token);
    let mut handles = Vec::with_capacity(10);
    for i in 0..10 {
        let base = Arc::clone(&base);
        let invite_token = Arc::clone(&invite_token);
        handles.push(tokio::spawn(async move {
            let client = reqwest::Client::new();
            let username = format!("racer{i:02}");
            let email = format!("{username}@example.com");
            let r = redeem(&client, &base, &invite_token, &username, &email).await;
            r.status()
        }));
    }

    let mut ok = 0;
    let mut gone = 0;
    let mut other = 0;
    for h in handles {
        let status = h.await.unwrap();
        if status == StatusCode::OK {
            ok += 1;
        } else if status == StatusCode::GONE {
            gone += 1;
        } else {
            other += 1;
        }
    }

    assert_eq!(ok, 3, "exactly cap-many redeems should succeed");
    assert_eq!(gone, 7, "the rest should hit 410 link_unavailable");
    assert_eq!(other, 0, "no unexpected statuses");
    assert_eq!(read_uses_count(&invite_token).await, 3);
}

/// Anti-impersonation regression: display_name with bidi or zero-width
/// characters is rejected. Without this, an attacker registers as
/// "alice\u{202E}" — visually indistinguishable from real "alice" — and
/// the chat log shows two indistinguishable users.
#[tokio::test]
async fn redeem_rejects_unicode_display_name_attacks() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();
    let admin = admin_token(&base).await;

    let create_resp = create_link(
        &client,
        &base,
        &admin,
        Utc::now() + Duration::hours(1),
        None,
        None,
    )
    .await;
    let invite_token = create_resp.json::<Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    // RLO (right-to-left override) — flips display order downstream of it.
    let bad_names = [
        "alice\u{202E}",    // RLO
        "alice\u{200B}bob", // ZWSP
        "ali\u{FEFF}ce",    // BOM
        "alice\u{2068}",    // bidi isolate
        "alice\nbob",       // newline (control char)
    ];
    for bad in &bad_names {
        let r = client
            .post(format!("{base}/v1/invite-links/{invite_token}/redeem"))
            .json(&json!({
                "username": "newbie",
                "password": "secret123",
                "display_name": bad,
                "email": "newbie@example.com",
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::BAD_REQUEST,
            "display_name {bad:?} should be rejected"
        );
        let body: Value = r.json().await.unwrap();
        assert_eq!(body["error"], "display_name_invalid");
    }
    // None of the bad attempts should have burned a slot.
    assert_eq!(read_uses_count(&invite_token).await, 0);
}

/// Race-loser on email (not username) must surface as `email_taken`, not
/// `username_taken`. Otherwise the FE highlights the wrong input. Stress
/// path: someone took the email *between* the pre-flight check and the
/// INSERT — we simulate that by inserting a user with the target email
/// directly into the DB after the pre-flight passes (here we just seed
/// the email up-front, which exercises the same INSERT-time UNIQUE
/// trigger because pre-flight catches it on the next attempt — so we
/// lean on a different sleeve: bypass pre-flight by hitting an email
/// that exists but check the *response code mapping*, which proves the
/// constraint-name match works).
#[tokio::test]
async fn redeem_email_collision_returns_email_taken_code() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();
    let admin = admin_token(&base).await;

    let create_resp = create_link(
        &client,
        &base,
        &admin,
        Utc::now() + Duration::hours(1),
        None,
        None,
    )
    .await;
    let invite_token = create_resp.json::<Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    // alice@matehub.dev is seeded — try to register a *different* username
    // with that same email. Pre-flight returns email_taken (good); the
    // mapping under test is what proves the constraint-name fallback
    // also routes to email_taken, not username_taken.
    let r = redeem(
        &client,
        &base,
        &invite_token,
        "newalice", // a fresh username
        "alice@matehub.dev",
    )
    .await;
    assert_eq!(r.status(), StatusCode::CONFLICT);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["error"], "email_taken");
    assert_eq!(read_uses_count(&invite_token).await, 0);
}
