//! Integration tests for the DM endpoints.
//!
//! Use real Postgres + the dev seed (Alice/Bob/Charlie in DEV_HUB_ID=1).

mod common;

use reqwest::Client;
use serde_json::{Value, json};

const HUB_ID: i64 = 1;
const ALICE_ID: &str = "1001";
const BOB_ID: &str = "1002";
const CHARLIE_ID: &str = "1003";

fn auth(token: &str) -> String {
    format!("Bearer {token}")
}

async fn setup() -> (String, Client, String, String, String) {
    let base = common::spawn_app().await;
    let client = Client::new();
    let alice = common::login(&base, "alice").await;
    let bob = common::login(&base, "bob").await;
    let charlie = common::login(&base, "charlie").await;
    (base, client, alice, bob, charlie)
}

// ── POST /dms ───────────────────────────────────────

#[tokio::test]
async fn open_dm_creates_channel_with_two_participants() {
    let (base, client, alice, _, _) = setup().await;

    let res = client
        .post(format!("{base}/v1/hubs/{HUB_ID}/dms"))
        .header("Authorization", auth(&alice))
        .json(&json!({"recipient_id": BOB_ID}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    let body: Value = res.json().await.unwrap();
    assert_eq!(body["type"], "dm");
    assert_eq!(body["name"], "");
    let participants = body["participants"].as_array().unwrap();
    assert_eq!(participants.len(), 2);
    let pset: std::collections::HashSet<&str> = participants
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(pset.contains(ALICE_ID));
    assert!(pset.contains(BOB_ID));
}

#[tokio::test]
async fn open_dm_is_idempotent() {
    let (base, client, alice, _, _) = setup().await;

    let r1: Value = client
        .post(format!("{base}/v1/hubs/{HUB_ID}/dms"))
        .header("Authorization", auth(&alice))
        .json(&json!({"recipient_id": BOB_ID}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let r2: Value = client
        .post(format!("{base}/v1/hubs/{HUB_ID}/dms"))
        .header("Authorization", auth(&alice))
        .json(&json!({"recipient_id": BOB_ID}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(r1["id"], r2["id"]);
}

#[tokio::test]
async fn open_dm_is_symmetric_across_callers() {
    // Alice → Bob and Bob → Alice resolve to the same channel.
    let (base, client, alice, bob, _) = setup().await;

    let from_alice: Value = client
        .post(format!("{base}/v1/hubs/{HUB_ID}/dms"))
        .header("Authorization", auth(&alice))
        .json(&json!({"recipient_id": BOB_ID}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let from_bob: Value = client
        .post(format!("{base}/v1/hubs/{HUB_ID}/dms"))
        .header("Authorization", auth(&bob))
        .json(&json!({"recipient_id": ALICE_ID}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(from_alice["id"], from_bob["id"]);
}

#[tokio::test]
async fn open_dm_with_self_is_rejected() {
    let (base, client, alice, _, _) = setup().await;

    let res = client
        .post(format!("{base}/v1/hubs/{HUB_ID}/dms"))
        .header("Authorization", auth(&alice))
        .json(&json!({"recipient_id": ALICE_ID}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn open_dm_with_non_member_is_rejected() {
    let (base, client, alice, _, _) = setup().await;

    let res = client
        .post(format!("{base}/v1/hubs/{HUB_ID}/dms"))
        .header("Authorization", auth(&alice))
        .json(&json!({"recipient_id": "999999"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

// ── GET /dms ────────────────────────────────────────

#[tokio::test]
async fn list_dms_returns_only_my_channels() {
    let (base, client, alice, bob, charlie) = setup().await;

    // Alice opens DM with Bob.
    client
        .post(format!("{base}/v1/hubs/{HUB_ID}/dms"))
        .header("Authorization", auth(&alice))
        .json(&json!({"recipient_id": BOB_ID}))
        .send()
        .await
        .unwrap();

    // Bob opens DM with Charlie.
    client
        .post(format!("{base}/v1/hubs/{HUB_ID}/dms"))
        .header("Authorization", auth(&bob))
        .json(&json!({"recipient_id": CHARLIE_ID}))
        .send()
        .await
        .unwrap();

    // Alice should see only Alice↔Bob.
    let alice_dms: Vec<Value> = client
        .get(format!("{base}/v1/hubs/{HUB_ID}/dms"))
        .header("Authorization", auth(&alice))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(alice_dms.len(), 1);
    let ps: std::collections::HashSet<&str> = alice_dms[0]["participants"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(ps.contains(ALICE_ID));
    assert!(ps.contains(BOB_ID));
    assert!(!ps.contains(CHARLIE_ID));

    // Charlie should see only Bob↔Charlie.
    let charlie_dms: Vec<Value> = client
        .get(format!("{base}/v1/hubs/{HUB_ID}/dms"))
        .header("Authorization", auth(&charlie))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(charlie_dms.len(), 1);
    let ps: std::collections::HashSet<&str> = charlie_dms[0]["participants"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(ps.contains(BOB_ID));
    assert!(ps.contains(CHARLIE_ID));
    assert!(!ps.contains(ALICE_ID));
}

// ── /channels listing must NOT leak DMs ─────────────

#[tokio::test]
async fn list_channels_excludes_dms() {
    let (base, client, alice, _, _) = setup().await;

    client
        .post(format!("{base}/v1/hubs/{HUB_ID}/dms"))
        .header("Authorization", auth(&alice))
        .json(&json!({"recipient_id": BOB_ID}))
        .send()
        .await
        .unwrap();

    let channels: Vec<Value> = client
        .get(format!("{base}/v1/hubs/{HUB_ID}/channels"))
        .header("Authorization", auth(&alice))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    for ch in &channels {
        assert_ne!(ch["type"], "dm", "DM channel leaked into /channels");
    }
}

// ── Generic create rejects type=dm ──────────────────

#[tokio::test]
async fn create_channel_rejects_dm_type() {
    let (base, client, alice, _, _) = setup().await;

    let res = client
        .post(format!("{base}/v1/hubs/{HUB_ID}/channels"))
        .header("Authorization", auth(&alice))
        .json(&json!({"name": "sneaky", "type": "dm"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}
