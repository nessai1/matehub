mod common;

use futures_util::{SinkExt, StreamExt};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const HUB_ID: i64 = 7001;
const CHANNEL_ID: i64 = 8001;

/// Helper: read next WS text frame, parse as JSON.
async fn next_json(ws: &mut tokio_tungstenite::WebSocketStream<impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>) -> Value {
    let msg = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next())
        .await
        .expect("timeout")
        .unwrap()
        .unwrap();
    serde_json::from_str(&msg.into_text().unwrap()).unwrap()
}

/// Helper: connect, receive HELLO, send IDENTIFY, receive READY.
/// Returns (ws_stream, session_id).
async fn identify(base: &str, token: &str) -> (tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, String) {
    let ws_url = common::ws_url(base, "/gateway");
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    // HELLO
    let hello = next_json(&mut ws).await;
    assert_eq!(hello["op"], 10);

    // IDENTIFY
    ws.send(Message::Text(
        json!({"op": 2, "d": {"token": token}}).to_string().into(),
    ))
    .await
    .unwrap();

    // READY
    let ready = next_json(&mut ws).await;
    assert_eq!(ready["op"], 0);
    assert_eq!(ready["t"], "READY");
    let session_id = ready["d"]["session_id"].as_str().unwrap().to_string();

    (ws, session_id)
}

#[tokio::test]
async fn gateway_sends_hello() {
    let base = common::spawn_app().await;
    let ws_url = common::ws_url(&base, "/gateway");

    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    let parsed = next_json(&mut ws).await;
    assert_eq!(parsed["op"], 10);
    assert!(parsed["d"]["heartbeat_interval"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn gateway_identify_and_receive_dispatch() {
    let base = common::spawn_app().await;
    let token = common::test_jwt("alice", HUB_ID);
    let client = reqwest::Client::new();

    let (mut ws, _session_id) = identify(&base, &token).await;

    // Give gateway time to subscribe to NATS
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Send a message via REST
    let resp = client
        .post(format!("{base}/v1/channels/{CHANNEL_ID}/messages"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({"content": "live message!"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Receive DISPATCH (op=0, t=MESSAGE_CREATE)
    let parsed = next_json(&mut ws).await;
    assert_eq!(parsed["op"], 0);
    assert_eq!(parsed["t"], "MESSAGE_CREATE");
    assert_eq!(parsed["d"]["content"], "live message!");
    assert!(parsed["s"].as_u64().unwrap() > 0, "sequence must be set");
}

#[tokio::test]
async fn gateway_invalid_token_gets_invalid_session() {
    let base = common::spawn_app().await;
    let ws_url = common::ws_url(&base, "/gateway");

    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    // HELLO
    let _ = next_json(&mut ws).await;

    // IDENTIFY with bad token
    ws.send(Message::Text(
        json!({"op": 2, "d": {"token": "invalid"}}).to_string().into(),
    ))
    .await
    .unwrap();

    // INVALID_SESSION (op=9)
    let parsed = next_json(&mut ws).await;
    assert_eq!(parsed["op"], 9);
}

#[tokio::test]
async fn gateway_heartbeat_ack() {
    let base = common::spawn_app().await;
    let token = common::test_jwt("bob", HUB_ID);

    let (mut ws, _session_id) = identify(&base, &token).await;

    // Send HEARTBEAT (op=1)
    ws.send(Message::Text(
        json!({"op": 1, "d": 0}).to_string().into(),
    ))
    .await
    .unwrap();

    // HEARTBEAT_ACK (op=11)
    let parsed = next_json(&mut ws).await;
    assert_eq!(parsed["op"], 11);
}

#[tokio::test]
async fn gateway_resume_replays_events() {
    let base = common::spawn_app().await;
    let token = common::test_jwt("charlie", HUB_ID);
    let client = reqwest::Client::new();

    // Connect and identify
    let (mut ws1, session_id) = identify(&base, &token).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Send 3 messages -> they get buffered with seq 1, 2, 3
    for i in 1..=3 {
        let resp = client
            .post(format!("{base}/v1/channels/{CHANNEL_ID}/messages"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&json!({"content": format!("msg-{i}")}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 201);
    }

    // Read all 3 dispatches
    let mut last_seq = 0u64;
    for _ in 0..3 {
        let ev = next_json(&mut ws1).await;
        assert_eq!(ev["op"], 0);
        last_seq = ev["s"].as_u64().unwrap();
    }
    assert_eq!(last_seq, 3);

    // Disconnect (close WS)
    let _ = ws1.close(None).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Reconnect with RESUME from seq=1 (should replay seq 2, 3)
    let ws_url = common::ws_url(&base, "/gateway");
    let (mut ws2, _) = connect_async(&ws_url).await.unwrap();

    // HELLO
    let hello = next_json(&mut ws2).await;
    assert_eq!(hello["op"], 10);

    // Send RESUME
    ws2.send(Message::Text(
        json!({"op": 6, "d": {"session_id": session_id, "seq": 1}})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();

    // Should receive 2 replayed events (seq 2 and 3)
    let replay1 = next_json(&mut ws2).await;
    assert_eq!(replay1["op"], 0);
    assert_eq!(replay1["s"], 2);

    let replay2 = next_json(&mut ws2).await;
    assert_eq!(replay2["op"], 0);
    assert_eq!(replay2["s"], 3);

    // Then RESUMED dispatch
    let resumed = next_json(&mut ws2).await;
    assert_eq!(resumed["op"], 0);
    assert_eq!(resumed["t"], "RESUMED");
}

#[tokio::test]
async fn gateway_resume_invalid_session_id() {
    let base = common::spawn_app().await;
    let ws_url = common::ws_url(&base, "/gateway");

    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    // HELLO
    let _ = next_json(&mut ws).await;

    // RESUME with bogus session_id
    ws.send(Message::Text(
        json!({"op": 6, "d": {"session_id": "nonexistent", "seq": 0}})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();

    // Should get INVALID_SESSION
    let parsed = next_json(&mut ws).await;
    assert_eq!(parsed["op"], 9);
}
