mod common;

use pretty_assertions::assert_eq;
use serde_json::{json, Value};

#[tokio::test]
async fn health_check() {
    let base = common::spawn_app().await;

    let resp = reqwest::get(format!("{base}/health")).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "ok");
}

#[tokio::test]
async fn create_session_returns_201() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/v1/sessions"))
        .json(&json!({"channel_id": "00000000-0000-0000-0000-000000000001"}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 201);

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["created"], true);
    assert!(body["session_id"].is_string());
    assert!(body["ws_url"].as_str().unwrap().contains("/ws/"));
}

#[tokio::test]
async fn create_session_idempotent() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();
    let channel_id = "00000000-0000-0000-0000-000000000002";

    // First call -- creates
    let resp1 = client
        .post(format!("{base}/v1/sessions"))
        .json(&json!({"channel_id": channel_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), 201);
    let body1: Value = resp1.json().await.unwrap();

    // Second call -- returns existing
    let resp2 = client
        .post(format!("{base}/v1/sessions"))
        .json(&json!({"channel_id": channel_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 200);
    let body2: Value = resp2.json().await.unwrap();

    assert_eq!(body1["session_id"], body2["session_id"]);
    assert_eq!(body2["created"], false);
}

#[tokio::test]
async fn get_session_returns_info() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();

    let create_resp: Value = client
        .post(format!("{base}/v1/sessions"))
        .json(&json!({"channel_id": "00000000-0000-0000-0000-000000000003"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let session_id = create_resp["session_id"].as_str().unwrap();

    let resp = client
        .get(format!("{base}/v1/sessions/{session_id}"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["session_id"], session_id);
    assert_eq!(body["channel_id"], "00000000-0000-0000-0000-000000000003");
    assert!(body["participants"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn get_nonexistent_session_returns_404() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "{base}/v1/sessions/00000000-0000-0000-0000-000000000099"
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 404);
}
