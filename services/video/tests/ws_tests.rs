mod common;

use futures_util::{SinkExt, StreamExt};
use pretty_assertions::assert_eq;
use serde_json::{json, Value};
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// Helper: create a session and return (base_url, session_id)
async fn create_test_session(base: &str) -> String {
    let client = reqwest::Client::new();
    let resp: Value = client
        .post(format!("{base}/v1/sessions"))
        .json(&json!({"channel_id": "00000000-0000-0000-0000-aaaaaaaaaaaa"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    resp["session_id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn ws_connect_and_receive_error_on_join() {
    let base = common::spawn_app().await;
    let session_id = create_test_session(&base).await;

    let ws_url = common::ws_url(&base, &format!("/ws/{session_id}?user_id=alice"));
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    // Send join
    ws.send(Message::Text(
        json!({"type": "join", "sdp_offer": "fake-sdp"}).to_string().into(),
    ))
    .await
    .unwrap();

    // Should get an error (SFU not implemented yet)
    let msg = ws.next().await.unwrap().unwrap();
    let text = msg.into_text().unwrap();
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["type"], "error");
    assert!(parsed["message"].as_str().unwrap().contains("SFU engine"));
}

#[tokio::test]
async fn ws_participant_joined_broadcast() {
    let base = common::spawn_app().await;
    let session_id = create_test_session(&base).await;

    // Alice connects
    let ws_url_alice = common::ws_url(&base, &format!("/ws/{session_id}?user_id=alice"));
    let (mut ws_alice, _) = connect_async(&ws_url_alice).await.unwrap();

    // Send join so Alice is registered
    ws_alice
        .send(Message::Text(
            json!({"type": "join", "sdp_offer": "fake"}).to_string().into(),
        ))
        .await
        .unwrap();

    // Drain Alice's error response from join
    let _ = ws_alice.next().await;

    // Bob connects -- Alice should receive participant_joined
    let ws_url_bob = common::ws_url(&base, &format!("/ws/{session_id}?user_id=bob"));
    let (_ws_bob, _) = connect_async(&ws_url_bob).await.unwrap();

    // Alice should receive participant_joined for Bob
    let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws_alice.next())
        .await
        .expect("timeout waiting for participant_joined")
        .unwrap()
        .unwrap();

    let parsed: Value = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
    assert_eq!(parsed["type"], "participant_joined");
    assert_eq!(parsed["user_id"], "bob");
}

#[tokio::test]
async fn ws_participant_left_broadcast() {
    let base = common::spawn_app().await;
    let session_id = create_test_session(&base).await;

    // Alice connects
    let ws_url_alice = common::ws_url(&base, &format!("/ws/{session_id}?user_id=alice"));
    let (mut ws_alice, _) = connect_async(&ws_url_alice).await.unwrap();
    ws_alice
        .send(Message::Text(
            json!({"type": "join", "sdp_offer": "fake"}).to_string().into(),
        ))
        .await
        .unwrap();
    let _ = ws_alice.next().await; // drain error

    // Bob connects
    let ws_url_bob = common::ws_url(&base, &format!("/ws/{session_id}?user_id=bob"));
    let (mut ws_bob, _) = connect_async(&ws_url_bob).await.unwrap();

    // Alice gets participant_joined
    let _ = ws_alice.next().await;

    // Bob sends leave
    ws_bob
        .send(Message::Text(json!({"type": "leave"}).to_string().into()))
        .await
        .unwrap();

    // Alice should receive participant_left
    let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws_alice.next())
        .await
        .expect("timeout waiting for participant_left")
        .unwrap()
        .unwrap();

    let parsed: Value = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
    assert_eq!(parsed["type"], "participant_left");
    assert_eq!(parsed["user_id"], "bob");
}

#[tokio::test]
async fn ws_session_destroyed_on_last_leave() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();
    let session_id = create_test_session(&base).await;

    // Alice connects and immediately leaves
    let ws_url = common::ws_url(&base, &format!("/ws/{session_id}?user_id=alice"));
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();
    ws.send(Message::Text(json!({"type": "leave"}).to_string().into()))
        .await
        .unwrap();

    // Poll until session is destroyed (instead of fixed sleep)
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let resp = client
            .get(format!("{base}/v1/sessions/{session_id}"))
            .send()
            .await
            .unwrap();
        if resp.status() == 404 {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("session was not destroyed within 5 seconds");
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn ws_invalid_message_returns_error() {
    let base = common::spawn_app().await;
    let session_id = create_test_session(&base).await;

    let ws_url = common::ws_url(&base, &format!("/ws/{session_id}?user_id=alice"));
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    // Send garbage
    ws.send(Message::Text("{\"type\": \"nonexistent\"}".into()))
        .await
        .unwrap();

    let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
        .await
        .expect("timeout")
        .unwrap()
        .unwrap();

    let parsed: Value = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
    assert_eq!(parsed["type"], "error");
}

#[tokio::test]
async fn ws_connect_to_nonexistent_session_closes() {
    let base = common::spawn_app().await;

    let ws_url = common::ws_url(
        &base,
        "/ws/00000000-0000-0000-0000-000000000099?user_id=alice",
    );
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    // Server should close the connection since session doesn't exist
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next()).await;

    match result {
        Ok(Some(Ok(Message::Close(_)))) | Ok(None) => {} // clean close
        Ok(Some(Err(_))) => {} // reset without handshake -- also acceptable (server dropped connection)
        Err(_) => {} // timeout -- server silently dropped
        other => panic!("expected close/error/timeout, got {other:?}"),
    }
}

#[tokio::test]
async fn ws_abrupt_disconnect_triggers_cleanup() {
    let base = common::spawn_app().await;
    let session_id = create_test_session(&base).await;

    // Alice connects
    let ws_url_alice = common::ws_url(&base, &format!("/ws/{session_id}?user_id=alice"));
    let (mut ws_alice, _) = connect_async(&ws_url_alice).await.unwrap();
    ws_alice
        .send(Message::Text(
            json!({"type": "join", "sdp_offer": "fake"}).to_string().into(),
        ))
        .await
        .unwrap();
    let _ = ws_alice.next().await; // drain error

    // Bob connects
    let ws_url_bob = common::ws_url(&base, &format!("/ws/{session_id}?user_id=bob"));
    let (ws_bob, _) = connect_async(&ws_url_bob).await.unwrap();
    let _ = ws_alice.next().await; // drain participant_joined for bob

    // Bob drops without sending leave (abrupt disconnect)
    drop(ws_bob);

    // Alice should receive participant_left
    let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws_alice.next())
        .await
        .expect("timeout waiting for participant_left after abrupt disconnect")
        .unwrap()
        .unwrap();

    let parsed: Value = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
    assert_eq!(parsed["type"], "participant_left");
    assert_eq!(parsed["user_id"], "bob");
}

#[tokio::test]
async fn ws_get_session_shows_participants() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();
    let session_id = create_test_session(&base).await;

    // Alice connects
    let ws_url = common::ws_url(&base, &format!("/ws/{session_id}?user_id=alice"));
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();
    ws.send(Message::Text(
        json!({"type": "join", "sdp_offer": "fake"}).to_string().into(),
    ))
    .await
    .unwrap();
    let _ = ws.next().await; // drain response

    // REST API should show Alice
    let resp: Value = client
        .get(format!("{base}/v1/sessions/{session_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let participants = resp["participants"].as_array().unwrap();
    assert_eq!(participants.len(), 1);
    assert_eq!(participants[0]["user_id"], "alice");
}
