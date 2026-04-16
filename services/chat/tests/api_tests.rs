mod common;

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

const HUB_ID: i64 = 1001;
const CHANNEL_ID: i64 = 2001;

#[tokio::test]
async fn send_message_and_get_history() {
    let base = common::spawn_app().await;
    let token = common::test_jwt("alice", HUB_ID);
    let client = reqwest::Client::new();

    // Send a message
    let resp = client
        .post(format!("{base}/v1/channels/{CHANNEL_ID}/messages"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({"content": "Hello ScyllaDB!"}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 201);
    let msg: Value = resp.json().await.unwrap();
    assert_eq!(msg["content"], "Hello ScyllaDB!");
    assert!(msg["message_id"].as_i64().unwrap() > 0);
    assert_eq!(msg["author_id"], "test-user-alice");

    // Get history
    let resp = client
        .get(format!("{base}/v1/channels/{CHANNEL_ID}/messages?limit=10"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let messages: Vec<Value> = resp.json().await.unwrap();
    assert!(!messages.is_empty(), "history should contain at least one message");
    assert_eq!(messages[0]["content"], "Hello ScyllaDB!");
}

#[tokio::test]
async fn send_message_requires_auth() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/v1/channels/{CHANNEL_ID}/messages"))
        .json(&json!({"content": "no auth"}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn empty_message_rejected() {
    let base = common::spawn_app().await;
    let token = common::test_jwt("bob", HUB_ID);
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/v1/channels/{CHANNEL_ID}/messages"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({"content": "   "}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
}

#[tokio::test]
#[ignore] // TODO: debug cursor pagination with ScyllaDB row deserialization
async fn cursor_pagination_works() {
    let base = common::spawn_app().await;
    let token = common::test_jwt("charlie", HUB_ID);
    let client = reqwest::Client::new();
    let channel = 3001i64;

    // Send 5 messages
    let mut ids = Vec::new();
    for i in 0..5 {
        let resp: Value = client
            .post(format!("{base}/v1/channels/{channel}/messages"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&json!({"content": format!("msg {i}")}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        ids.push(resp["message_id"].as_i64().unwrap());
    }

    // Get last 3 messages
    let messages: Vec<Value> = client
        .get(format!("{base}/v1/channels/{channel}/messages?limit=3"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(messages.len(), 3);

    // First message should be the newest (DESC order)
    assert_eq!(messages[0]["message_id"].as_i64().unwrap(), ids[4]);

    // Paginate: get messages before the oldest in current page
    let before_id = messages[2]["message_id"].as_i64().unwrap();
    let older: Vec<Value> = client
        .get(format!(
            "{base}/v1/channels/{channel}/messages?limit=3&before={before_id}"
        ))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(older.len(), 2, "should have 2 older messages");
    assert_eq!(older[0]["message_id"].as_i64().unwrap(), ids[1]);
    assert_eq!(older[1]["message_id"].as_i64().unwrap(), ids[0]);
}

#[tokio::test]
async fn mentions_are_parsed() {
    let base = common::spawn_app().await;
    let token = common::test_jwt("alice", HUB_ID);
    let client = reqwest::Client::new();
    let channel = 4001i64;

    let resp: Value = client
        .post(format!("{base}/v1/channels/{channel}/messages"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({"content": "Hey @bob and @charlie check this out"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let mentions = resp["mentions"].as_array().unwrap();
    assert!(mentions.contains(&json!("bob")));
    assert!(mentions.contains(&json!("charlie")));
}

#[tokio::test]
async fn mention_everyone_detected() {
    let base = common::spawn_app().await;
    let token = common::test_jwt("alice", HUB_ID);
    let client = reqwest::Client::new();
    let channel = 5001i64;

    let resp: Value = client
        .post(format!("{base}/v1/channels/{channel}/messages"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({"content": "Attention @everyone!"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(resp["mention_everyone"], true);
}

#[tokio::test]
async fn snowflake_ids_are_monotonic() {
    let base = common::spawn_app().await;
    let token = common::test_jwt("alice", HUB_ID);
    let client = reqwest::Client::new();
    let channel = 6001i64;

    let mut prev_id = 0i64;
    for i in 0..5 {
        let resp: Value = client
            .post(format!("{base}/v1/channels/{channel}/messages"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&json!({"content": format!("msg {i}")}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        let id = resp["message_id"].as_i64().unwrap();
        assert!(id > prev_id, "Snowflake IDs must increase: {prev_id} -> {id}");
        prev_id = id;
    }
}
