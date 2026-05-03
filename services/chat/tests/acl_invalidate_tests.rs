//! End-to-end test for the acl.invalidate consumer.
//!
//! Skips itself when NATS / Redis aren't reachable so it's safe to run in
//! environments without infra (e.g. plain `cargo test` on a fresh checkout).

use redis::AsyncCommands;
use serde_json::json;

#[tokio::test]
async fn channel_invalidate_drops_matching_keys() {
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());

    let Ok(nats) = async_nats::connect(&nats_url).await else {
        eprintln!("[acl_invalidate] NATS unavailable, skipping");
        return;
    };
    let Ok(client) = redis::Client::open(redis_url.as_str()) else {
        eprintln!("[acl_invalidate] Redis client init failed, skipping");
        return;
    };
    let Ok(mut conn) = client.get_multiplexed_async_connection().await else {
        eprintln!("[acl_invalidate] Redis connect failed, skipping");
        return;
    };

    // Seed the cache.
    let key_a = "acl:42:1001:7777:r";
    let key_b = "acl:42:1001:8888:w";
    let key_unrelated = "acl:99:1001:7777:r"; // different hub — must survive
    let _: () = conn.set_ex(key_a, 1i32, 60).await.unwrap();
    let _: () = conn.set_ex(key_b, 0i32, 60).await.unwrap();
    let _: () = conn.set_ex(key_unrelated, 1i32, 60).await.unwrap();

    // Spawn the consumer.
    matehub_chat::acl_consumer::spawn(nats.clone(), Some(conn.clone()));
    // Subscription handshake.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // Publish the invalidation: hub_id 42, channel_id 1001.
    let payload = json!({
        "scope": "channel",
        "hub_id": "42",
        "channel_id": "1001",
    });
    nats.publish(
        "acl.invalidate",
        serde_json::to_vec(&payload).unwrap().into(),
    )
    .await
    .unwrap();

    // SCAN+DEL is fire-and-forget — give it a moment.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let a_exists: bool = conn.exists(key_a).await.unwrap();
    let b_exists: bool = conn.exists(key_b).await.unwrap();
    let u_exists: bool = conn.exists(key_unrelated).await.unwrap();

    // Cleanup the survivor regardless of assertion outcome.
    let _: () = conn.del(key_unrelated).await.unwrap_or(());

    assert!(!a_exists, "acl:42:1001:7777:r should have been deleted");
    assert!(!b_exists, "acl:42:1001:8888:w should have been deleted");
    assert!(u_exists, "unrelated hub key must NOT be touched");
}

#[tokio::test]
async fn user_invalidate_drops_user_keys_only() {
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());

    let Ok(nats) = async_nats::connect(&nats_url).await else {
        return;
    };
    let Ok(client) = redis::Client::open(redis_url.as_str()) else {
        return;
    };
    let Ok(mut conn) = client.get_multiplexed_async_connection().await else {
        return;
    };

    let key_target = "acl:42:5555:1001:r";
    let key_other_user = "acl:42:5555:2002:r";
    let _: () = conn.set_ex(key_target, 1i32, 60).await.unwrap();
    let _: () = conn.set_ex(key_other_user, 1i32, 60).await.unwrap();

    matehub_chat::acl_consumer::spawn(nats.clone(), Some(conn.clone()));
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let payload = json!({
        "scope": "user",
        "hub_id": "42",
        "user_id": "1001",
    });
    nats.publish(
        "acl.invalidate",
        serde_json::to_vec(&payload).unwrap().into(),
    )
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let target_exists: bool = conn.exists(key_target).await.unwrap();
    let other_exists: bool = conn.exists(key_other_user).await.unwrap();

    let _: () = conn.del(key_other_user).await.unwrap_or(());

    assert!(!target_exists, "user 1001 key should have been deleted");
    assert!(other_exists, "user 2002 key must NOT be touched");
}
