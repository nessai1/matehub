//! Transcoding job protocol shared between chat service (publisher)
//! and transcoder service (worker).
//!
//! Transport: NATS JetStream with queue group for work distribution.
//!
//! Stream config:
//!   name: "TRANSCODE"
//!   subjects: ["transcode.request", "transcode.result"]
//!   retention: WorkQueue (request) / Interest (result)
//!   max_age: 1 hour
//!   replicas: 1 (dev) / 3 (prod)
//!
//! Consumer config (for transcoder workers):
//!   durable_name: "transcode-worker"
//!   ack_wait: 120s  (transcode can take a while)
//!   max_deliver: 3  (retry failed jobs 3x then DLQ)
//!   filter_subject: "transcode.request"

use serde::{Deserialize, Serialize};

pub const SUBJECT_REQUEST: &str = "transcode.request";
pub const SUBJECT_RESULT: &str = "transcode.result";
pub const STREAM_NAME: &str = "TRANSCODE";
pub const CONSUMER_NAME: &str = "transcode-worker";

/// Job for the transcoder pool. Chat service publishes this on video upload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscodeRequest {
    /// Unique attachment ID (used as idempotency key).
    pub attachment_id: String,
    /// Hub + channel + message IDs (for updating the persisted message).
    pub hub_id: i64,
    pub channel_id: i64,
    pub message_id: i64,
    pub bucket: i32,
    /// Source file S3 key (full path, as returned by upload).
    pub source_key: String,
    /// Original MIME (video/quicktime, video/x-msvideo, etc.).
    pub source_mime: String,
    /// Original filename for UX.
    pub source_name: String,
    /// Target format preset. Phase 1: "mp4_h264_aac" only.
    pub target_preset: String,
}

/// Result broadcast by transcoder on completion (or failure).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscodeResult {
    pub attachment_id: String,
    pub hub_id: i64,
    pub channel_id: i64,
    pub message_id: i64,
    pub bucket: i32,
    pub status: TranscodeStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum TranscodeStatus {
    /// Transcoding succeeded. Replace attachment URL + metadata.
    #[serde(rename = "ok")]
    Ok {
        /// New S3 URL (transcoded mp4).
        url: String,
        content_type: String,
        width: Option<u32>,
        height: Option<u32>,
        duration: Option<f32>,
        size: u64,
    },
    /// Transcoding failed. Mark attachment as failed.
    #[serde(rename = "failed")]
    Failed { error: String },
}
