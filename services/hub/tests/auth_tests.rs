mod common;

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

#[tokio::test]
async fn login_with_username() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/v1/auth/login"))
        .json(&json!({"login": "alice", "password": "123123", "hub_id": "def00000-0000-0000-0000-000000000001"}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["username"], "alice");
    assert!(body["token"].as_str().unwrap().len() > 20);
    assert!(body["user_id"].is_string());
    assert_eq!(body["hub_slug"], "dev-hub");
}

#[tokio::test]
async fn login_with_email() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/v1/auth/login"))
        .json(&json!({"login": "alice@matehub.dev", "password": "123123", "hub_id": "def00000-0000-0000-0000-000000000001"}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["username"], "alice");
}

#[tokio::test]
async fn login_wrong_password() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/v1/auth/login"))
        .json(&json!({"login": "alice", "password": "wrong", "hub_id": "def00000-0000-0000-0000-000000000001"}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn login_nonexistent_user() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/v1/auth/login"))
        .json(&json!({"login": "nobody", "password": "123123", "hub_id": "def00000-0000-0000-0000-000000000001"}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn login_wrong_hub() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/v1/auth/login"))
        .json(&json!({"login": "alice", "password": "123123", "hub_id": "00000000-0000-0000-0000-000000000099"}))
        .send()
        .await
        .unwrap();

    // alice exists but not a member of this hub
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn login_returns_avatar_url() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();

    let resp: Value = client
        .post(format!("{base}/v1/auth/login"))
        .json(&json!({"login": "bob", "password": "123123", "hub_id": "def00000-0000-0000-0000-000000000001"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // avatar_url is nullable
    assert!(resp["avatar_url"].is_null() || resp["avatar_url"].is_string());
}
