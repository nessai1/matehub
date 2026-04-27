//! End-to-end test for DM ringing.
//!
//! Alice opens a DM with Bob via the hub crate (helpers below), connects both
//! to the chat WS gateway, then POSTs to the chat /call/{start,decline,cancel}
//! endpoints. We assert that the recipient sees the dispatch and a third
//! party (Charlie) does not.

mod common;

use futures_util::{SinkExt, StreamExt};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use matehub_hub::db::seed::{DEV_HUB_ID, DEV_USER_ALICE, DEV_USER_BOB};

/// Open a DM Alice↔Bob directly in PG, returning the channel id.
async fn open_dm_alice_bob(pg: &sqlx::PgPool) -> i64 {
    use matehub_common::snowflake;
    let pair_key = format!("{}:{}", DEV_USER_ALICE.min(DEV_USER_BOB), DEV_USER_ALICE.max(DEV_USER_BOB));

    sqlx::query(
        "INSERT INTO channels (id, hub_id, name, type, position, dm_pair_key)
         VALUES ($1, $2, '', 'dm', 0, $3)
         ON CONFLICT (hub_id, dm_pair_key) WHERE dm_pair_key IS NOT NULL DO NOTHING",
    )
    .bind(snowflake::next_id())
    .bind(DEV_HUB_ID)
    .bind(&pair_key)
    .execute(pg)
    .await
    .unwrap();

    let channel_id: i64 = sqlx::query_scalar(
        "SELECT id FROM channels WHERE hub_id = $1 AND dm_pair_key = $2",
    )
    .bind(DEV_HUB_ID)
    .bind(&pair_key)
    .fetch_one(pg)
    .await
    .unwrap();

    for uid in &[DEV_USER_ALICE, DEV_USER_BOB] {
        sqlx::query(
            "INSERT INTO dm_participants (channel_id, user_id) VALUES ($1, $2)
             ON CONFLICT DO NOTHING",
        )
        .bind(channel_id)
        .bind(uid)
        .execute(pg)
        .await
        .unwrap();
    }

    channel_id
}

async fn next_json(
    ws: &mut tokio_tungstenite::WebSocketStream<
        impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    >,
) -> Value {
    let msg = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next())
        .await
        .expect("timeout")
        .unwrap()
        .unwrap();
    serde_json::from_str(&msg.into_text().unwrap()).unwrap()
}

async fn try_next_json(
    ws: &mut tokio_tungstenite::WebSocketStream<
        impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    >,
    timeout_ms: u64,
) -> Option<Value> {
    let msg = tokio::time::timeout(
        std::time::Duration::from_millis(timeout_ms),
        ws.next(),
    )
    .await
    .ok()??
    .ok()?;
    serde_json::from_str(&msg.into_text().ok()?).ok()
}

async fn identify(
    base: &str,
    token: &str,
) -> tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
> {
    let ws_url = common::ws_url(base, "/gateway");
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();
    let hello = next_json(&mut ws).await;
    assert_eq!(hello["op"], 10);
    ws.send(Message::Text(
        json!({"op": 2, "d": {"token": token}}).to_string().into(),
    ))
    .await
    .unwrap();
    let ready = next_json(&mut ws).await;
    assert_eq!(ready["t"], "READY");
    ws
}

#[tokio::test]
async fn dm_call_invite_reaches_recipient_only() {
    let base = common::spawn_app().await;
    let pg_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://matehub:matehub-dev@localhost:5432/matehub_test".into());
    let pg = matehub_hub::db::connect(&pg_url).await.unwrap();
    let dm_channel = open_dm_alice_bob(&pg).await;

    let alice_token = common::test_jwt("alice", DEV_HUB_ID);
    let bob_token = common::test_jwt("bob", DEV_HUB_ID);
    let charlie_token = common::test_jwt("charlie", DEV_HUB_ID);

    let mut alice_ws = identify(&base, &alice_token).await;
    let mut bob_ws = identify(&base, &bob_token).await;
    let mut charlie_ws = identify(&base, &charlie_token).await;

    // Subscriptions take a moment to attach.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Alice rings Bob.
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/v1/dms/{dm_channel}/call/start"))
        .header("Authorization", format!("Bearer {alice_token}"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    // Bob receives the invite.
    let invite = next_json(&mut bob_ws).await;
    assert_eq!(invite["t"], "DM_CALL_INVITE");
    assert_eq!(
        invite["d"]["from_user_id"].as_str().unwrap(),
        DEV_USER_ALICE.to_string()
    );
    assert_eq!(
        invite["d"]["channel_id"].as_str().unwrap(),
        dm_channel.to_string()
    );

    // Alice also receives it (she's a participant) — that's expected; the
    // frontend ignores invites it sent itself by comparing from_user_id.
    let echo = next_json(&mut alice_ws).await;
    assert_eq!(echo["t"], "DM_CALL_INVITE");

    // Charlie does NOT.
    let leak = try_next_json(&mut charlie_ws, 400).await;
    assert!(leak.is_none(), "DM call leaked to non-participant: {leak:?}");
}

#[tokio::test]
async fn dm_call_decline_round_trips_to_inviter() {
    let base = common::spawn_app().await;
    let pg_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://matehub:matehub-dev@localhost:5432/matehub_test".into());
    let pg = matehub_hub::db::connect(&pg_url).await.unwrap();
    let dm_channel = open_dm_alice_bob(&pg).await;

    let alice_token = common::test_jwt("alice", DEV_HUB_ID);
    let bob_token = common::test_jwt("bob", DEV_HUB_ID);

    let mut alice_ws = identify(&base, &alice_token).await;
    let _bob_ws = identify(&base, &bob_token).await;

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/v1/dms/{dm_channel}/call/decline"))
        .header("Authorization", format!("Bearer {bob_token}"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    let event = next_json(&mut alice_ws).await;
    assert_eq!(event["t"], "DM_CALL_DECLINE");
    assert_eq!(
        event["d"]["from_user_id"].as_str().unwrap(),
        DEV_USER_BOB.to_string()
    );
}

#[tokio::test]
async fn dm_call_endpoints_reject_non_participant() {
    let base = common::spawn_app().await;
    let pg_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://matehub:matehub-dev@localhost:5432/matehub_test".into());
    let pg = matehub_hub::db::connect(&pg_url).await.unwrap();
    let dm_channel = open_dm_alice_bob(&pg).await;

    let charlie_token = common::test_jwt("charlie", DEV_HUB_ID);
    let client = reqwest::Client::new();

    for path in ["start", "decline", "cancel"] {
        let resp = client
            .post(format!("{base}/v1/dms/{dm_channel}/call/{path}"))
            .header("Authorization", format!("Bearer {charlie_token}"))
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 403, "/{path} should be forbidden for non-participant");
    }
}
