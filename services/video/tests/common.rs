use tokio::net::TcpListener;
use tokio::sync::mpsc;

use matehub_video::sfu::SfuCommand;
use matehub_video::signaling::ServerMessage;

/// Spawns the video service HTTP+WS on a random port.
/// SFU commands are handled by a mock that responds to Join with an error
/// (no real str0m in test mode).
pub async fn spawn_app() -> String {
    let (sfu_cmd_tx, mut sfu_cmd_rx) = mpsc::unbounded_channel();

    // Mock SFU: responds to Join, drains everything else
    tokio::spawn(async move {
        while let Some(cmd) = sfu_cmd_rx.recv().await {
            if let SfuCommand::Join { reply_tx, .. } = cmd {
                let _ = reply_tx.send(ServerMessage::Error {
                    message: "SFU engine not available in test mode".into(),
                });
            }
        }
    });

    let state = matehub_video::state::AppState::new(sfu_cmd_tx);
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
