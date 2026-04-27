use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
    body::Body,
};
use axum_extra::extract::Multipart;
use bytes::{BufMut, Bytes, BytesMut};
use futures_util::stream;
use matehub_common::storage::UploadError;

use crate::api::AppState;
use crate::attachment::{Attachment, AttachmentStatus};
use crate::auth::AuthUser;
use matehub_common::snowflake;

const MAX_FILE_SIZE: usize = 1024 * 1024 * 1024; // 1 GB
/// Max bytes we'll fully buffer for images (needed to probe width/height).
/// Larger images still upload via streaming but skip dimension extraction.
const MAX_IMAGE_BUFFER: usize = 32 * 1024 * 1024; // 32 MB

// Denylist of clearly hostile mime types. Anything else flows through — chat
// users expect to drop docx/xlsx/keynote/sketch/whatever and have it round-trip.
// S3 stores opaquely, browser doesn't auto-execute downloads.
const BLOCKED_MIMES: &[&str] = &[
    "application/x-msdownload",     // .exe / .dll
    "application/x-ms-installer",   // .msi
    "application/x-msi",
    "application/x-bat",            // .bat
    "application/x-sh",             // .sh
    "application/x-csh",
    "application/x-msdos-program",
    "application/vnd.microsoft.portable-executable",
    "application/x-mach-binary",    // mach-o
    "application/x-elf",
    "application/x-executable",
];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/channels/{channel_id}/attachments",
            post(upload_attachment),
        )
        .route(
            "/v1/attachments/{attachment_id}/stream",
            get(stream_attachment),
        )
        // Bump body limit above MAX_FILE_SIZE so oversized uploads get 413 from us,
        // not 400 from axum's default 2MB multipart guard.
        .layer(DefaultBodyLimit::max(MAX_FILE_SIZE + 1024 * 1024))
}

async fn upload_attachment(
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    auth: AuthUser,
    mut multipart: Multipart,
) -> Result<Json<Attachment>, StatusCode> {
    let hub_id = auth.0.hub_id;

    if !crate::access::check(&state, hub_id, channel_id, auth.0.sub, crate::access::Action::Write).await {
        return Err(StatusCode::FORBIDDEN);
    }

    let field = multipart
        .next_field()
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?
        .ok_or(StatusCode::BAD_REQUEST)?;

    let file_name = field.file_name().unwrap_or("file").to_string();
    let content_type = field
        .content_type()
        .unwrap_or("application/octet-stream")
        .to_string();

    if BLOCKED_MIMES.iter().any(|m| content_type.starts_with(m)) {
        tracing::warn!(%content_type, "rejected blocked file type");
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    let s3 = state.s3.as_ref().ok_or_else(|| {
        tracing::error!("S3 not configured");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    let ext = file_name.rsplit('.').next().unwrap_or("bin");
    let attachment_id = snowflake::next_id();
    let rand_suffix: u64 = rand::random();
    let key = format!("{hub_id}/{channel_id}/{attachment_id}_{rand_suffix:016x}.{ext}");

    let is_image = content_type.starts_with("image/");

    // For images we tee the first MAX_IMAGE_BUFFER bytes into memory so we can
    // probe dimensions; anything past that still streams to S3 without buffering.
    // For non-images we just stream directly -- upload_stream controls RAM.
    let image_probe: std::sync::Arc<parking_lot::Mutex<Option<BytesMut>>> =
        std::sync::Arc::new(parking_lot::Mutex::new(if is_image {
            Some(BytesMut::with_capacity(std::cmp::min(MAX_IMAGE_BUFFER, 1 << 20)))
        } else {
            None
        }));

    // Adapt Field (which exposes async chunk() -> Result<Option<Bytes>>) into a Stream.
    // Side-effect: accumulate into image_probe up to MAX_IMAGE_BUFFER.
    let probe_handle = image_probe.clone();
    let field_stream = stream::unfold(field, move |mut f| {
        let probe = probe_handle.clone();
        async move {
            match f.chunk().await {
                Ok(Some(chunk)) => {
                    if let Some(buf) = probe.lock().as_mut() {
                        let remaining = MAX_IMAGE_BUFFER.saturating_sub(buf.len());
                        if remaining > 0 {
                            let take = std::cmp::min(remaining, chunk.len());
                            buf.put_slice(&chunk[..take]);
                        }
                    }
                    Some((Ok::<Bytes, axum_extra::extract::multipart::MultipartError>(chunk), f))
                }
                Ok(None) => None,
                Err(e) => Some((Err(e), f)),
            }
        }
    });

    let (url, size) = match s3
        .upload_stream(&key, &content_type, Box::pin(field_stream), MAX_FILE_SIZE)
        .await
    {
        Ok(ok) => ok,
        Err(UploadError::TooLarge { .. }) => return Err(StatusCode::PAYLOAD_TOO_LARGE),
        Err(UploadError::Empty) => return Err(StatusCode::BAD_REQUEST),
        Err(UploadError::Stream(e)) => {
            tracing::warn!(%content_type, "client stream error: {e}");
            return Err(StatusCode::BAD_REQUEST);
        }
        Err(UploadError::S3(e)) => {
            tracing::error!("S3 upload failed: {e}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Extract image dimensions from buffered prefix (best effort).
    let (width, height) = if is_image {
        let buf = image_probe.lock().take();
        buf.map(|b| extract_image_dimensions(&b)).unwrap_or((None, None))
    } else {
        (None, None)
    };

    let needs_transcode = Attachment::needs_transcode(&content_type);
    let status = if needs_transcode {
        AttachmentStatus::Transcoding
    } else {
        AttachmentStatus::Ready
    };

    let attachment = Attachment {
        id: attachment_id.to_string(),
        url,
        content_type,
        name: file_name,
        size: size as u64,
        width,
        height,
        duration: None,
        thumb_url: None,
        status,
    };

    tracing::info!(
        %hub_id, %channel_id, id = %attachment.id,
        name = %attachment.name, size = attachment.size,
        "attachment uploaded"
    );

    Ok(Json(attachment))
}

/// Extract width/height from image bytes without loading full pixel data.
fn extract_image_dimensions(data: &[u8]) -> (Option<u32>, Option<u32>) {
    match image::ImageReader::new(std::io::Cursor::new(data)).with_guessed_format() {
        Ok(reader) => match reader.into_dimensions() {
            Ok((w, h)) => (Some(w), Some(h)),
            Err(_) => (None, None),
        },
        Err(_) => (None, None),
    }
}

/// Stream attachment with HTTP Range support (for video player seek + lazy load).
/// Proxies S3 with Range header forwarding.
///
/// Flow:
///   1. Look up the attachment metadata by id (sidecar `attachments` table).
///   2. Verify the caller's JWT hub_id matches the attachment's hub_id.
///   3. Issue a reqwest GET to the stored S3 URL, forwarding the client's
///      Range header verbatim. S3 answers with 206 + a partial body.
///   4. Stream the reqwest response body through to the client, mirroring
///      Content-Type / Content-Length / Content-Range / Accept-Ranges.
///
/// Why proxy instead of 302→S3 direct: (a) auth stays server-side — leaked
/// attachment ids don't bypass hub membership; (b) S3 endpoint/bucket layout
/// stays hidden from the client; (c) later switch to a private bucket +
/// presigned URLs is one handler change, not a client-wide update.
async fn stream_attachment(
    State(state): State<AppState>,
    Path(attachment_id): Path<String>,
    auth: AuthUser,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let row = state
        .data
        .get_attachment_index(&attachment_id)
        .await
        .map_err(|e| {
            tracing::error!(%attachment_id, "attachment index read failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    // Authz: the JWT must be scoped to the same hub the attachment lives in.
    if row.hub_id != auth.0.hub_id {
        tracing::warn!(
            %attachment_id,
            claim_hub = auth.0.hub_id,
            row_hub = row.hub_id,
            "cross-hub attachment access denied"
        );
        return Err(StatusCode::FORBIDDEN);
    }

    // Plus per-channel ACL: an attachment in someone else's DM is not yours
    // to fetch even if you happen to know the snowflake id.
    if !crate::access::check(&state, row.hub_id, row.channel_id, auth.0.sub, crate::access::Action::Read).await {
        return Err(StatusCode::FORBIDDEN);
    }

    let mut req_builder = reqwest::Client::new().get(&row.url);
    if let Some(range) = headers.get(header::RANGE) {
        if let Ok(s) = range.to_str() {
            req_builder = req_builder.header(header::RANGE, s);
        }
    }

    let s3_resp = req_builder.send().await.map_err(|e| {
        tracing::error!(%attachment_id, url = %row.url, "S3 fetch failed: {e}");
        StatusCode::BAD_GATEWAY
    })?;

    let status = StatusCode::from_u16(s3_resp.status().as_u16())
        .unwrap_or(StatusCode::BAD_GATEWAY);

    // Pass through the headers S3 set so the browser video element knows
    // how to drive range requests. We don't blindly forward every header —
    // that leaks S3 metadata (ETag, AMZ request ids, etc). Only the ones
    // the media stack actually reads.
    let mut out_headers = HeaderMap::new();
    out_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&row.content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    out_headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    for name in [header::CONTENT_LENGTH, header::CONTENT_RANGE, header::LAST_MODIFIED] {
        if let Some(v) = s3_resp.headers().get(&name) {
            out_headers.insert(name, v.clone());
        }
    }

    // reqwest response body → axum body as a bytes stream. Memory is bounded
    // by reqwest's own read buffer, not the file size — 1 GB video doesn't
    // pin 1 GB of RAM.
    let body = Body::from_stream(s3_resp.bytes_stream());

    Ok((status, out_headers, body).into_response())
}
