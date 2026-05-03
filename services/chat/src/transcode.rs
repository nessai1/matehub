//! Transcode integration: publish requests, consume results.
//!
//! Architecture:
//! - On video upload (needs_transcode): chat publishes TranscodeRequest to NATS.
//! - Transcoder service picks from queue, does ffmpeg, publishes TranscodeResult.
//! - This module spawns a background task subscribed to transcode.result.>.
//! - On result: update the message in ScyllaDB, broadcast ATTACHMENT_UPDATED via fanout.
//!
//! Failure modes covered:
//! - Transcoder crash during work → NATS redelivers via ack_wait.
//! - NATS cluster down → publisher returns error (caller decides).
//! - Zombie attachments stuck in "transcoding" → reaper (separate background task).

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_nats::jetstream::{self, stream};
use futures_util::StreamExt;
use matehub_common::transcode::{
    STREAM_NAME, SUBJECT_REQUEST, SUBJECT_RESULT, TranscodeRequest, TranscodeResult,
    TranscodeStatus,
};

use crate::attachment::{Attachment, AttachmentStatus};
use crate::data_service::DataService;
use crate::fanout::FanoutService;

/// Publish a transcode request to the queue.
pub async fn publish_request(nats: &async_nats::Client, req: &TranscodeRequest) -> Result<()> {
    let js = jetstream::new(nats.clone());

    // Idempotent stream creation
    js.get_or_create_stream(stream::Config {
        name: STREAM_NAME.to_string(),
        subjects: vec![SUBJECT_REQUEST.to_string(), format!("{SUBJECT_RESULT}.*")],
        retention: stream::RetentionPolicy::WorkQueue,
        max_age: Duration::from_secs(3600),
        num_replicas: 1,
        ..Default::default()
    })
    .await
    .context("create/get stream")?;

    let payload = serde_json::to_vec(req)?;
    js.publish(SUBJECT_REQUEST.to_string(), payload.into())
        .await
        .context("publish")?
        .await
        .context("ack")?;

    tracing::info!(attachment_id = %req.attachment_id, "transcode request published");
    Ok(())
}

/// Spawn background task that consumes transcode.result.* events.
/// Updates message attachments in DB and broadcasts WS event.
pub fn spawn_result_consumer(
    nats: async_nats::Client,
    data: Arc<DataService>,
    fanout: Arc<FanoutService>,
) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = run_consumer(&nats, &data, &fanout).await {
                tracing::error!("transcode result consumer error: {e}. Retrying in 10s...");
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    });
}

async fn run_consumer(
    nats: &async_nats::Client,
    data: &Arc<DataService>,
    fanout: &Arc<FanoutService>,
) -> Result<()> {
    // Subscribe to all result events (wildcarded by hub_id)
    let subject = format!("{SUBJECT_RESULT}.*");
    let mut sub = nats.subscribe(subject.clone()).await?;
    tracing::info!(%subject, "transcode result consumer started");

    while let Some(msg) = sub.next().await {
        let result: TranscodeResult = match serde_json::from_slice(&msg.payload) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("bad result payload: {e}");
                continue;
            }
        };

        if let Err(e) = handle_result(data, fanout, result).await {
            tracing::error!("handle_result failed: {e:?}");
        }
    }
    Ok(())
}

async fn handle_result(
    data: &Arc<DataService>,
    fanout: &Arc<FanoutService>,
    result: TranscodeResult,
) -> Result<()> {
    let TranscodeResult {
        attachment_id,
        hub_id,
        channel_id,
        message_id,
        bucket,
        status,
    } = result;

    // Read current attachments
    let mut attachments = data
        .get_message_attachments(hub_id, channel_id, bucket, message_id)
        .await?
        .unwrap_or_default();

    let mut found = false;
    for a in &mut attachments {
        if a.id == attachment_id {
            match &status {
                TranscodeStatus::Ok {
                    url,
                    content_type,
                    width,
                    height,
                    duration,
                    size,
                } => {
                    a.url = url.clone();
                    a.content_type = content_type.clone();
                    if let Some(w) = width {
                        a.width = Some(*w);
                    }
                    if let Some(h) = height {
                        a.height = Some(*h);
                    }
                    if let Some(d) = duration {
                        a.duration = Some(*d);
                    }
                    a.size = *size;
                    a.status = AttachmentStatus::Ready;
                }
                TranscodeStatus::Failed { error: _ } => {
                    a.status = AttachmentStatus::Failed;
                }
            }
            found = true;
            break;
        }
    }

    if !found {
        tracing::warn!(%attachment_id, "result for unknown attachment");
        return Ok(());
    }

    // Persist
    data.update_attachments(hub_id, channel_id, bucket, message_id, &attachments)
        .await?;

    // Keep the streaming-proxy index in sync: URL/content-type/size just
    // changed from the source file to the transcoded output.
    if let TranscodeStatus::Ok {
        url,
        content_type,
        size,
        ..
    } = &status
    {
        if let Err(e) = data
            .update_attachment_index_row(&attachment_id, url, content_type, *size as i64)
            .await
        {
            tracing::error!(%attachment_id, "attachment index update failed: {e}");
        }
    }

    // Broadcast WS event so clients refresh
    let payload = serde_json::json!({
        "message_id": message_id.to_string(),
        "channel_id": channel_id.to_string(),
        "attachment_id": attachment_id,
        "attachments": attachments,
    });
    fanout
        .publish_event(hub_id, channel_id, "attachment_updated", &payload)
        .await;

    tracing::info!(%attachment_id, ?status, "attachment updated");
    Ok(())
}

/// Background reaper: marks attachments stuck in "transcoding" > 1h as "failed".
/// Runs every 5 minutes.
#[allow(dead_code)]
pub fn spawn_reaper(_data: Arc<DataService>) {
    // TODO: Phase 2 -- requires index on status + updated_at, or periodic scan.
    // For MVP, zombies self-heal on next message edit or via manual admin action.
}

/// Build a TranscodeRequest from an uploaded attachment that needs transcoding.
pub fn make_request(
    attachment: &Attachment,
    source_key: String,
    hub_id: i64,
    channel_id: i64,
    message_id: i64,
    bucket: i32,
) -> TranscodeRequest {
    TranscodeRequest {
        attachment_id: attachment.id.clone(),
        hub_id,
        channel_id,
        message_id,
        bucket,
        source_key,
        source_mime: attachment.content_type.clone(),
        source_name: attachment.name.clone(),
        target_preset: "mp4_h264_aac".to_string(),
    }
}
