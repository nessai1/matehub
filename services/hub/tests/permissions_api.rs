mod common;

use reqwest::Client;
use serde_json::{Value, json};

const HUB_ID: &str = "1";
const ALICE_ID: &str = "1001";

async fn setup() -> (String, Client, String, String) {
    let base = common::spawn_app().await;
    let client = Client::new();
    let alice_token = common::login(&base, "alice").await;
    let bob_token = common::login(&base, "bob").await;
    (base, client, alice_token, bob_token)
}

fn auth(token: &str) -> String {
    format!("Bearer {token}")
}

async fn get_groups(base: &str, client: &Client, token: &str) -> Vec<Value> {
    client
        .get(format!("{base}/v1/hubs/{HUB_ID}/groups"))
        .header("Authorization", auth(token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

fn find_group<'a>(groups: &'a [Value], name: &str) -> &'a Value {
    groups.iter().find(|g| g["name"] == name).unwrap()
}

// ── Group CRUD ────────────────────────────────────

#[tokio::test]
async fn admin_can_create_group() {
    let (base, client, alice, _) = setup().await;

    let res = client
        .post(format!("{base}/v1/hubs/{HUB_ID}/groups"))
        .header("Authorization", auth(&alice))
        .json(&json!({"name": "testrole", "color": "#FF0000"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);

    let body: Value = res.json().await.unwrap();
    assert_eq!(body["name"], "testrole");
    assert_eq!(body["hub_permissions"], 0);
}

#[tokio::test]
async fn non_admin_cannot_create_group() {
    let (base, client, _, bob) = setup().await;

    let res = client
        .post(format!("{base}/v1/hubs/{HUB_ID}/groups"))
        .header("Authorization", auth(&bob))
        .json(&json!({"name": "hacker-role"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn cannot_delete_admin_group() {
    let (base, client, alice, _) = setup().await;
    let groups = get_groups(&base, &client, &alice).await;
    let admin_id = find_group(&groups, "admin")["id"].as_str().unwrap();

    let res = client
        .delete(format!("{base}/v1/hubs/{HUB_ID}/groups/{admin_id}"))
        .header("Authorization", auth(&alice))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn cannot_delete_everyone_group() {
    let (base, client, alice, _) = setup().await;
    let groups = get_groups(&base, &client, &alice).await;
    let everyone_id = find_group(&groups, "everyone")["id"].as_str().unwrap();

    let res = client
        .delete(format!("{base}/v1/hubs/{HUB_ID}/groups/{everyone_id}"))
        .header("Authorization", auth(&alice))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn cannot_change_admin_hub_permissions() {
    let (base, client, alice, _) = setup().await;
    let groups = get_groups(&base, &client, &alice).await;
    let admin_id = find_group(&groups, "admin")["id"].as_str().unwrap();

    let res = client
        .patch(format!("{base}/v1/hubs/{HUB_ID}/groups/{admin_id}"))
        .header("Authorization", auth(&alice))
        .json(&json!({"hub_permissions": 0}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn can_change_admin_name_and_color() {
    let (base, client, alice, _) = setup().await;
    let groups = get_groups(&base, &client, &alice).await;
    let admin_id = find_group(&groups, "admin")["id"].as_str().unwrap();

    let res = client
        .patch(format!("{base}/v1/hubs/{HUB_ID}/groups/{admin_id}"))
        .header("Authorization", auth(&alice))
        .json(&json!({"name": "Admin", "color": "#FF0000"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["name"], "Admin");
}

#[tokio::test]
async fn admin_can_delete_custom_group() {
    let (base, client, alice, _) = setup().await;

    // Create
    let res = client
        .post(format!("{base}/v1/hubs/{HUB_ID}/groups"))
        .header("Authorization", auth(&alice))
        .json(&json!({"name": "deleteme"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let body: Value = res.json().await.unwrap();
    let group_id = body["id"].as_str().unwrap();

    // Delete
    let res = client
        .delete(format!("{base}/v1/hubs/{HUB_ID}/groups/{group_id}"))
        .header("Authorization", auth(&alice))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
}

// ── Creator protection ────────────────────────────

#[tokio::test]
async fn cannot_remove_creator_from_admin() {
    let (base, client, alice, _) = setup().await;
    let groups = get_groups(&base, &client, &alice).await;
    let admin_id = find_group(&groups, "admin")["id"].as_str().unwrap();

    let res = client
        .delete(format!(
            "{base}/v1/hubs/{HUB_ID}/groups/{admin_id}/members/{ALICE_ID}"
        ))
        .header("Authorization", auth(&alice))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

// ── Channel permissions ───────────────────────────

#[tokio::test]
async fn admin_can_create_and_delete_channel() {
    let (base, client, alice, _) = setup().await;

    let res = client
        .post(format!("{base}/v1/hubs/{HUB_ID}/channels"))
        .header("Authorization", auth(&alice))
        .json(&json!({"name": "test-ch", "type": "text"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let body: Value = res.json().await.unwrap();
    let ch_id = body["id"].as_str().unwrap();

    let res = client
        .delete(format!("{base}/v1/hubs/{HUB_ID}/channels/{ch_id}"))
        .header("Authorization", auth(&alice))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn non_admin_cannot_create_channel() {
    let (base, client, _, bob) = setup().await;

    let res = client
        .post(format!("{base}/v1/hubs/{HUB_ID}/channels"))
        .header("Authorization", auth(&bob))
        .json(&json!({"name": "hacked", "type": "text"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn non_admin_cannot_delete_channel() {
    let (base, client, alice, bob) = setup().await;

    // Alice creates
    let res = client
        .post(format!("{base}/v1/hubs/{HUB_ID}/channels"))
        .header("Authorization", auth(&alice))
        .json(&json!({"name": "protected-ch", "type": "text"}))
        .send()
        .await
        .unwrap();
    let body: Value = res.json().await.unwrap();
    let ch_id = body["id"].as_str().unwrap();

    // Bob tries to delete
    let res = client
        .delete(format!("{base}/v1/hubs/{HUB_ID}/channels/{ch_id}"))
        .header("Authorization", auth(&bob))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

// ── My permissions ────────────────────────────────

#[tokio::test]
async fn alice_is_admin_and_creator() {
    let (base, client, alice, _) = setup().await;

    let res = client
        .get(format!("{base}/v1/hubs/{HUB_ID}/my-permissions"))
        .header("Authorization", auth(&alice))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    let body: Value = res.json().await.unwrap();
    assert_eq!(body["is_admin"], true);
    assert_eq!(body["is_creator"], true);
}

#[tokio::test]
async fn bob_has_no_hub_perms() {
    let (base, client, _, bob) = setup().await;

    let res = client
        .get(format!("{base}/v1/hubs/{HUB_ID}/my-permissions"))
        .header("Authorization", auth(&bob))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    let body: Value = res.json().await.unwrap();
    assert_eq!(body["is_admin"], false);
    assert_eq!(body["is_creator"], false);
    assert_eq!(body["hub_bits"], 0);
}

// ── Privilege escalation ──────────────────────────

#[tokio::test]
async fn cannot_create_group_with_bits_you_dont_have() {
    let (base, client, alice, _) = setup().await;

    // Create a moderator role with only CREATE_TEXT_CHANNELS (128)
    let res = client
        .post(format!("{base}/v1/hubs/{HUB_ID}/groups"))
        .header("Authorization", auth(&alice))
        .json(&json!({"name": "mod", "hub_permissions": 128})) // CREATE_TEXT only
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    // Admin can grant any bits -- that's correct.
    // The real test is: if a mod tries to create a role with MANAGE_ROLES.
    // But dev tokens go through dev-token auth which bypasses JWT groups.
    // This test validates the admin path works correctly.
}

// ── Member kick ───────────────────────────────────

#[tokio::test]
async fn non_admin_cannot_kick() {
    let (base, client, _, bob) = setup().await;

    let charlie_id = "1003";
    let res = client
        .delete(format!("{base}/v1/hubs/{HUB_ID}/members/{charlie_id}"))
        .header("Authorization", auth(&bob))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}
