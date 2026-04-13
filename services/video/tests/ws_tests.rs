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

#[tokio::test]
async fn ws_ice_candidate_does_not_crash() {
    let base = common::spawn_app().await;
    let session_id = create_test_session(&base).await;

    let ws_url = common::ws_url(&base, &format!("/ws/{session_id}?user_id=alice"));
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    // Send join first
    ws.send(Message::Text(
        json!({"type": "join", "sdp_offer": "fake"}).to_string().into(),
    ))
    .await
    .unwrap();
    let _ = ws.next().await; // drain response

    // Send ICE candidate
    ws.send(Message::Text(
        json!({
            "type": "ice_candidate",
            "candidate": "candidate:1 1 udp 2130706431 192.168.1.1 5000 typ host",
            "sdp_mid": "0",
            "sdp_mline_index": 0
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();

    // Connection should still be alive -- send leave
    ws.send(Message::Text(json!({"type": "leave"}).to_string().into()))
        .await
        .unwrap();
}

#[tokio::test]
async fn ws_answer_does_not_crash() {
    let base = common::spawn_app().await;
    let session_id = create_test_session(&base).await;

    let ws_url = common::ws_url(&base, &format!("/ws/{session_id}?user_id=alice"));
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    ws.send(Message::Text(
        json!({"type": "join", "sdp_offer": "fake"}).to_string().into(),
    ))
    .await
    .unwrap();
    let _ = ws.next().await;

    // Send answer (even though no offer was sent -- should not crash)
    ws.send(Message::Text(
        json!({"type": "answer", "sdp_answer": "v=0\r\n"}).to_string().into(),
    ))
    .await
    .unwrap();

    ws.send(Message::Text(json!({"type": "leave"}).to_string().into()))
        .await
        .unwrap();
}

#[tokio::test]
async fn ws_duplicate_user_id_creates_separate_participants() {
    let base = common::spawn_app().await;
    let client = reqwest::Client::new();
    let session_id = create_test_session(&base).await;

    // Two connections with same user_id
    let ws_url = common::ws_url(&base, &format!("/ws/{session_id}?user_id=alice"));
    let (mut ws1, _) = connect_async(&ws_url).await.unwrap();
    let (mut ws2, _) = connect_async(&ws_url).await.unwrap();

    // Both join
    ws1.send(Message::Text(
        json!({"type": "join", "sdp_offer": "fake"}).to_string().into(),
    ))
    .await
    .unwrap();
    let _ = ws1.next().await; // drain error

    ws2.send(Message::Text(
        json!({"type": "join", "sdp_offer": "fake"}).to_string().into(),
    ))
    .await
    .unwrap();
    let _ = ws2.next().await;

    // REST should show 2 participants
    let resp: Value = client
        .get(format!("{base}/v1/sessions/{session_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let participants = resp["participants"].as_array().unwrap();
    assert_eq!(participants.len(), 2);

    // Both should have user_id "alice" but different participant_ids
    assert_eq!(participants[0]["user_id"], "alice");
    assert_eq!(participants[1]["user_id"], "alice");

    let ids: std::collections::HashSet<_> = participants
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 2, "participant IDs should be unique");
}

// ── Mute signaling tests (Stage 2) ──────────────

#[tokio::test]
async fn ws_mute_changed_broadcasts_to_others() {
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
    let _ = ws_alice.next().await; // drain participant_joined for bob

    // Drain Bob's participant_joined for Alice
    let _ = ws_bob.next().await;

    // Bob enables camera
    ws_bob
        .send(Message::Text(
            json!({"type": "mute_changed", "kind": "video", "muted": false})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();

    // Alice should receive participant_muted
    let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws_alice.next())
        .await
        .expect("timeout waiting for participant_muted")
        .unwrap()
        .unwrap();

    let parsed: Value = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
    assert_eq!(parsed["type"], "participant_muted");
    assert_eq!(parsed["kind"], "video");
    assert_eq!(parsed["muted"], false);
    assert_eq!(parsed["user_id"], Value::Null); // no user_id in this message
}

#[tokio::test]
async fn ws_mute_changed_not_echoed_to_sender() {
    let base = common::spawn_app().await;
    let session_id = create_test_session(&base).await;

    // Alice connects alone
    let ws_url = common::ws_url(&base, &format!("/ws/{session_id}?user_id=alice"));
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();
    ws.send(Message::Text(
        json!({"type": "join", "sdp_offer": "fake"}).to_string().into(),
    ))
    .await
    .unwrap();
    let _ = ws.next().await; // drain error

    // Alice sends mute_changed
    ws.send(Message::Text(
        json!({"type": "mute_changed", "kind": "video", "muted": false})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();

    // Alice should NOT receive her own mute event (timeout = no message)
    let result =
        tokio::time::timeout(std::time::Duration::from_millis(200), ws.next()).await;
    assert!(result.is_err(), "sender should not receive own mute_changed");
}

#[tokio::test]
async fn ws_mute_state_sent_on_join() {
    let base = common::spawn_app().await;
    let session_id = create_test_session(&base).await;

    // Bob connects and enables camera
    let ws_url_bob = common::ws_url(&base, &format!("/ws/{session_id}?user_id=bob"));
    let (mut ws_bob, _) = connect_async(&ws_url_bob).await.unwrap();
    ws_bob
        .send(Message::Text(
            json!({"type": "mute_changed", "kind": "video", "muted": false})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();

    // Alice connects -- should receive participant_joined AND participant_muted for Bob
    let ws_url_alice = common::ws_url(&base, &format!("/ws/{session_id}?user_id=alice"));
    let (mut ws_alice, _) = connect_async(&ws_url_alice).await.unwrap();

    // First message: participant_joined for Bob
    let msg1 = tokio::time::timeout(std::time::Duration::from_secs(2), ws_alice.next())
        .await
        .expect("timeout waiting for participant_joined")
        .unwrap()
        .unwrap();
    let p1: Value = serde_json::from_str(&msg1.into_text().unwrap()).unwrap();
    assert_eq!(p1["type"], "participant_joined");
    assert_eq!(p1["user_id"], "bob");

    // Second message: participant_muted (Bob's camera is on)
    let msg2 = tokio::time::timeout(std::time::Duration::from_secs(2), ws_alice.next())
        .await
        .expect("timeout waiting for participant_muted")
        .unwrap()
        .unwrap();
    let p2: Value = serde_json::from_str(&msg2.into_text().unwrap()).unwrap();
    assert_eq!(p2["type"], "participant_muted");
    assert_eq!(p2["kind"], "video");
    assert_eq!(p2["muted"], false);
}

#[tokio::test]
async fn ws_mute_state_not_sent_when_muted() {
    let base = common::spawn_app().await;
    let session_id = create_test_session(&base).await;

    // Bob connects but does NOT enable camera (default: muted)
    let ws_url_bob = common::ws_url(&base, &format!("/ws/{session_id}?user_id=bob"));
    let (_ws_bob, _) = connect_async(&ws_url_bob).await.unwrap();

    // Alice connects -- should receive participant_joined but NO participant_muted
    let ws_url_alice = common::ws_url(&base, &format!("/ws/{session_id}?user_id=alice"));
    let (mut ws_alice, _) = connect_async(&ws_url_alice).await.unwrap();

    // First message: participant_joined for Bob
    let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws_alice.next())
        .await
        .expect("timeout waiting for participant_joined")
        .unwrap()
        .unwrap();
    let parsed: Value = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
    assert_eq!(parsed["type"], "participant_joined");

    // No second message (Bob's camera is off by default)
    let result =
        tokio::time::timeout(std::time::Duration::from_millis(200), ws_alice.next()).await;
    assert!(
        result.is_err(),
        "should not receive participant_muted when camera is off"
    );
}

#[tokio::test]
async fn ws_mute_toggle_sequence() {
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
    let _ = ws_alice.next().await; // drain participant_joined
    let _ = ws_bob.next().await; // drain participant_joined for alice

    // Bob: camera on -> off -> on
    for muted in [false, true, false] {
        ws_bob
            .send(Message::Text(
                json!({"type": "mute_changed", "kind": "video", "muted": muted})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();

        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws_alice.next())
            .await
            .expect("timeout")
            .unwrap()
            .unwrap();
        let parsed: Value = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
        assert_eq!(parsed["type"], "participant_muted");
        assert_eq!(parsed["muted"], muted);
    }
}

#[tokio::test]
async fn ws_binary_frame_ignored() {
    let base = common::spawn_app().await;
    let session_id = create_test_session(&base).await;

    let ws_url = common::ws_url(&base, &format!("/ws/{session_id}?user_id=alice"));
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    // Send binary frame -- should be ignored, not crash
    ws.send(Message::Binary(vec![0x00, 0x01, 0x02].into()))
        .await
        .unwrap();

    // Connection should still work
    ws.send(Message::Text(
        json!({"type": "join", "sdp_offer": "fake"}).to_string().into(),
    ))
    .await
    .unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let parsed: Value = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
    assert_eq!(parsed["type"], "error"); // SFU not available in test
}
