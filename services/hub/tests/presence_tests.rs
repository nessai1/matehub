mod common;

use futures_util::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::Value;
use tokio_tungstenite::connect_async;

const HUB_ID: &str = "1";

#[tokio::test]
async fn presence_ws_connects_with_jwt() {
    let base = common::spawn_app().await;
    let token = common::login(&base, "alice").await;

    let ws_url = common::ws_url(
        &base,
        &format!("/ws/presence/{HUB_ID}?token={token}"),
    );

    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    // Should stay connected -- send close
    ws.close(None).await.unwrap();
}

#[tokio::test]
async fn presence_ws_rejects_bad_token() {
    let base = common::spawn_app().await;

    let ws_url = common::ws_url(
        &base,
        &format!("/ws/presence/{HUB_ID}?token=invalid-token"),
    );

    // Should fail to upgrade or close immediately
    let result = connect_async(&ws_url).await;
    assert!(result.is_err(), "should reject invalid token");
}

#[tokio::test]
async fn presence_makes_user_online() {
    let base = common::spawn_app().await;
    let token = common::login(&base, "alice").await;
    let client = reqwest::Client::new();

    // Connect presence WS
    let ws_url = common::ws_url(
        &base,
        &format!("/ws/presence/{HUB_ID}?token={token}"),
    );
    let (_ws, _) = connect_async(&ws_url).await.unwrap();

    // Give Redis time to set the key
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Check members -- alice should be online
    let members: Vec<Value> = client
        .get(format!("{base}/v1/hubs/{HUB_ID}/members-full"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let alice = members.iter().find(|m| m["username"] == "alice").unwrap();
    assert_eq!(alice["is_online"], true, "alice should be online while WS connected");
}

#[tokio::test]
async fn presence_offline_after_disconnect() {
    let base = common::spawn_app().await;
    let token = common::login(&base, "bob").await;
    let client = reqwest::Client::new();

    // Connect and immediately disconnect
    let ws_url = common::ws_url(
        &base,
        &format!("/ws/presence/{HUB_ID}?token={token}"),
    );
    let (ws, _) = connect_async(&ws_url).await.unwrap();
    drop(ws);

    // Wait for server to process disconnect
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let members: Vec<Value> = client
        .get(format!("{base}/v1/hubs/{HUB_ID}/members-full"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let bob = members.iter().find(|m| m["username"] == "bob").unwrap();
    assert_eq!(bob["is_online"], false, "bob should be offline after WS disconnect");
}
