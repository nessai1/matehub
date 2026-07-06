use thiserror::Error;

#[derive(Debug, Error)]
pub enum RtcClientError {
    #[error("websocket connect failed: {0}")]
    WsConnect(String),

    #[error("signaling channel closed")]
    SignalingClosed,

    #[error("timed out waiting for {0}")]
    Timeout(&'static str),

    #[error("SDP negotiation failed: {0}")]
    Sdp(String),

    #[error("SFU rejected request: {0}")]
    Server(String),

    #[error("session API call failed: {0}")]
    SessionApi(String),

    #[error("engine is not running")]
    EngineGone,

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
