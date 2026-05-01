mod common;

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

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
        .json(&json!({"channel_id": "9000000000000001", "hub_id": "1"}))
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
    let channel_id = "9000000000000002";

    // First call -- creates
    let resp1 = client
        .post(format!("{base}/v1/sessions"))
        .json(&json!({"channel_id": channel_id, "hub_id": "1"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), 201);
    let body1: Value = resp1.json().await.unwrap();

    // Second call -- returns existing
    let resp2 = client
        .post(format!("{base}/v1/sessions"))
        .json(&json!({"channel_id": channel_id, "hub_id": "1"}))
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
        .json(&json!({"channel_id": "9000000000000003", "hub_id": "1"}))
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
    assert_eq!(body["channel_id"], "9000000000000003");
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

#[tokio::test]
async fn create_session_empty_body_returns_4xx() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/v1/sessions"))
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();

    assert!(resp.status().is_client_error());
}

#[tokio::test]
async fn create_session_invalid_uuid_returns_4xx() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/v1/sessions"))
        .json(&json!({"channel_id": "not-a-uuid"}))
        .send()
        .await
        .unwrap();

    assert!(resp.status().is_client_error());
}

#[tokio::test]
async fn create_session_no_content_type_returns_4xx() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/v1/sessions"))
        .body(r#"{"channel_id": "9000000000000001", "hub_id": "1"}"#)
        .send()
        .await
        .unwrap();

    assert!(resp.status().is_client_error());
}

#[tokio::test]
async fn get_session_malformed_uuid_returns_4xx() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{base}/v1/sessions/not-a-uuid"))
        .send()
        .await
        .unwrap();

    assert!(resp.status().is_client_error());
}

#[tokio::test]
async fn different_channels_create_different_sessions() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();

    let resp1: Value = client
        .post(format!("{base}/v1/sessions"))
        .json(&json!({"channel_id": "9000000000000010", "hub_id": "1"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let resp2: Value = client
        .post(format!("{base}/v1/sessions"))
        .json(&json!({"channel_id": "9000000000000020", "hub_id": "1"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_ne!(
        resp1["session_id"].as_str().unwrap(),
        resp2["session_id"].as_str().unwrap()
    );
}

#[tokio::test]
async fn create_session_ws_url_contains_host() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();

    let resp: Value = client
        .post(format!("{base}/v1/sessions"))
        .json(&json!({"channel_id": "9000000000000030", "hub_id": "1"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let ws_url = resp["ws_url"].as_str().unwrap();
    assert!(
        ws_url.starts_with("ws://"),
        "ws_url should start with ws://"
    );
    assert!(
        ws_url.contains(&format!("/ws/{}", resp["session_id"].as_str().unwrap())),
        "ws_url should contain /ws/session_id"
    );
    // ws_url should NOT contain localhost:4000 (hardcoded), but the actual test server address
    assert!(
        !ws_url.contains("localhost:4000"),
        "ws_url should not be hardcoded to localhost:4000"
    );
}

#[tokio::test]
async fn concurrent_session_creation_same_channel() {
    let base = common::spawn_app().await;
    let channel_id = "9000000000000042";

    let futs = (0..10).map(|_| {
        let base = base.clone();
        let channel_id = channel_id.to_string();
        async move {
            reqwest::Client::new()
                .post(format!("{base}/v1/sessions"))
                .json(&serde_json::json!({"channel_id": channel_id, "hub_id": "1"}))
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    });

    let results: Vec<Value> = futures_util::future::join_all(futs).await;
    let session_ids: std::collections::HashSet<_> = results
        .iter()
        .map(|r| r["session_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        session_ids.len(),
        1,
        "all concurrent requests should return the same session_id"
    );
}
