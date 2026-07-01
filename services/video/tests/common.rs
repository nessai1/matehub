use tokio::net::TcpListener;

use matehub_video::sfu::{SfuCommand, SfuPool};
use matehub_video::signaling::ServerMessage;

/// Spawns the video service HTTP+WS on a random port.
/// SFU commands are handled by a mock that responds to Join with an error
/// (no real str0m in test mode).
pub async fn spawn_app() -> String {
    // Mock single-shard pool: tests don't need real media threads, just a
    // sender we can feed into the AppState and a receiver to drain.
    let (sfu_cmd_tx, sfu_cmd_rx) = crossbeam::channel::bounded::<SfuCommand>(8192);
    let pool = SfuPool::for_test(vec![sfu_cmd_tx]);

    // Mock SFU: responds to Join, drains everything else.
    std::thread::spawn(move || {
        while let Ok(cmd) = sfu_cmd_rx.recv() {
            if let SfuCommand::Join { reply_tx, .. } = cmd {
                let _ = reply_tx.try_send(ServerMessage::Error {
                    message: "SFU engine not available in test mode".into(),
                });
            }
        }
    });

    // No NATS in tests — voice-occupancy publish is a no-op.
    // No ROOM_DEBUG capture in tests either.
    let state = matehub_video::state::AppState::new(pool, None, None);
    let app = matehub_video::api::routes(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://{addr}")
}

#[allow(dead_code)] // used by ws_tests, not by api_tests
pub fn ws_url(http_base: &str, path: &str) -> String {
    http_base.replace("http://", "ws://") + path
}
