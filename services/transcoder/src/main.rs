//! MateHub transcoder service.
//!
//! Pulls video transcode jobs from NATS JetStream queue.
//! Runs ffmpeg to convert to browser-compatible MP4 (H.264 + AAC).
//! Uploads result to S3, publishes result event.
//!
//! Horizontal scaling: deploy N replicas, NATS queue group distributes work.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_nats::jetstream::{self, consumer, stream};
use futures_util::StreamExt;
use matehub_common::storage::S3Storage;
use matehub_common::transcode::{
    CONSUMER_NAME, STREAM_NAME, SUBJECT_REQUEST, SUBJECT_RESULT, TranscodeRequest, TranscodeResult,
    TranscodeStatus,
};

mod ffmpeg;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::from_path("../../.env")
        .or_else(|_| dotenvy::from_path(".env"))
        .or_else(|_| dotenvy::dotenv().map(|_| ()));

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Verify ffmpeg is available
    ffmpeg::check_ffmpeg().await?;

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let nats = async_nats::connect(&nats_url)
        .await
        .context("NATS connect")?;
    tracing::info!(%nats_url, "NATS connected");

    let js = jetstream::new(nats.clone());

    // Ensure stream exists (idempotent)
    let _stream = js
        .get_or_create_stream(stream::Config {
            name: STREAM_NAME.to_string(),
            subjects: vec![SUBJECT_REQUEST.to_string(), format!("{SUBJECT_RESULT}.*")],
            retention: stream::RetentionPolicy::WorkQueue,
            max_age: Duration::from_secs(3600),
            num_replicas: 1, // TODO: 3 in prod
            ..Default::default()
        })
        .await
        .context("create stream")?;

    tracing::info!(stream = STREAM_NAME, "stream ready");

    // Ensure consumer exists (durable, pull-based, shared queue group)
    let stream_handle = js.get_stream(STREAM_NAME).await?;
    let consumer_handle: consumer::PullConsumer = stream_handle
        .get_or_create_consumer(
            CONSUMER_NAME,
            consumer::pull::Config {
                durable_name: Some(CONSUMER_NAME.to_string()),
                filter_subject: SUBJECT_REQUEST.to_string(),
                ack_wait: Duration::from_secs(120),
                max_deliver: 3,
                ..Default::default()
            },
        )
        .await
        .context("create consumer")?;

    tracing::info!(consumer = CONSUMER_NAME, "consumer ready, waiting for jobs");

    // S3 storage
    let s3 = Arc::new(S3Storage::from_env("S3_BUCKET_CHAT_MEDIA").await);

    // Pull messages
    let mut messages = consumer_handle
        .stream()
        .max_messages_per_batch(1)
        .messages()
        .await?;

    while let Some(msg) = messages.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("pull error: {e}");
                continue;
            }
        };

        let req: TranscodeRequest = match serde_json::from_slice(&msg.payload) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("invalid payload: {e}");
                let _ = msg.ack().await;
                continue;
            }
        };

        let attachment_id = req.attachment_id.clone();
        let hub_id = req.hub_id;
        let channel_id = req.channel_id;
        let message_id = req.message_id;
        let bucket = req.bucket;

        tracing::info!(%attachment_id, mime = %req.source_mime, "starting transcode");

        // Signal "in progress" -- prevents NATS redelivery while ffmpeg runs long
        let inp_handle = msg.clone();
        let inp_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                if inp_handle.ack_with(async_nats::jetstream::AckKind::Progress).await.is_err() {
                    break;
                }
            }
        });

        let result = process(&s3, req).await;
        inp_task.abort();

        let status = match result {
            Ok(ok) => ok,
            Err(e) => {
                tracing::error!(%attachment_id, "transcode failed: {e:?}");
                TranscodeStatus::Failed { error: e.to_string() }
            }
        };

        let result_event = TranscodeResult {
            attachment_id: attachment_id.clone(),
            hub_id,
            channel_id,
            message_id,
            bucket,
            status,
        };

        let subject = format!("{SUBJECT_RESULT}.{hub_id}");
        let payload = serde_json::to_vec(&result_event).unwrap();
        if let Err(e) = js.publish(subject.clone(), payload.into()).await {
            tracing::error!(%attachment_id, "failed to publish result: {e}");
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(%attachment_id, "ACK failed: {e}");
        } else {
            tracing::info!(%attachment_id, "transcode complete");
        }
    }

    Ok(())
}

async fn process(
    s3: &S3Storage,
    req: TranscodeRequest,
) -> Result<TranscodeStatus> {
    // Download source from S3 to tmp
    let tmp_dir = std::env::temp_dir();
    let src_path: PathBuf = tmp_dir.join(format!("in_{}.bin", req.attachment_id));
    let dst_path: PathBuf = tmp_dir.join(format!("out_{}.mp4", req.attachment_id));

    // Use source S3 URL for download (public bucket)
    let src_url = s3.public_url(&req.source_key);
    let client = reqwest::Client::new();
    let bytes = client
        .get(&src_url)
        .send()
        .await
        .context("download source")?
        .bytes()
        .await?;
    tokio::fs::write(&src_path, &bytes).await?;

    // Transcode with ffmpeg
    let info = ffmpeg::transcode_to_mp4(&src_path, &dst_path).await?;

    // Upload result
    let transcoded_bytes = tokio::fs::read(&dst_path).await?;
    let out_key = format!(
        "{}/{}/{}_transcoded.mp4",
        req.hub_id, req.channel_id, req.attachment_id
    );
    let new_url = s3
        .upload(&out_key, "video/mp4", transcoded_bytes.clone())
        .await?;

    // Cleanup tmp files
    let _ = tokio::fs::remove_file(&src_path).await;
    let _ = tokio::fs::remove_file(&dst_path).await;

    Ok(TranscodeStatus::Ok {
        url: new_url,
        content_type: "video/mp4".to_string(),
        width: info.width,
        height: info.height,
        duration: info.duration,
        size: transcoded_bytes.len() as u64,
    })
}
