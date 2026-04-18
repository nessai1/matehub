use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use axum_extra::extract::Multipart;
use serde::Serialize;

use crate::api::AppState;
use crate::auth::AuthUser;
use crate::snowflake;

const MAX_FILE_SIZE: usize = 25 * 1024 * 1024; // 25 MB free tier

const ALLOWED_MIMES: &[&str] = &[
    "image/jpeg", "image/png", "image/gif", "image/webp",
    "video/mp4", "video/webm",
    "audio/mpeg", "audio/ogg", "audio/wav",
    "application/pdf", "text/plain", "application/zip",
];

#[derive(Serialize)]
struct AttachmentResponse {
    attachment_id: String,
    url: String,
    content_type: String,
    name: String,
    size: usize,
}

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/v1/channels/{channel_id}/attachments",
        post(upload_attachment),
    )
}

async fn upload_attachment(
    State(state): State<AppState>,
    Path(channel_id_raw): Path<String>,
    auth: AuthUser,
    mut multipart: Multipart,
) -> Result<Json<AttachmentResponse>, StatusCode> {
    let hub_id = crate::api::str_to_i64(&auth.0.hub_id);
    let channel_id = crate::api::str_to_i64(&channel_id_raw);

    let field = multipart
        .next_field()
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?
        .ok_or(StatusCode::BAD_REQUEST)?;

    let file_name = field
        .file_name()
        .unwrap_or("file")
        .to_string();

    let content_type = field
        .content_type()
        .unwrap_or("application/octet-stream")
        .to_string();

    // Validate MIME type
    if !ALLOWED_MIMES.iter().any(|m| content_type.starts_with(m)) {
        tracing::warn!(%content_type, "rejected file type");
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    let data = field
        .bytes()
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    if data.len() > MAX_FILE_SIZE {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }

    let ext = file_name
        .rsplit('.')
        .next()
        .unwrap_or("bin");

    let attachment_id = snowflake::next_id();
    let rand_suffix: u64 = rand::random();
    let key = format!("{hub_id}/{channel_id}/{attachment_id}_{rand_suffix:016x}.{ext}");

    // Upload to S3
    let s3 = state.s3.as_ref().ok_or_else(|| {
        tracing::error!("S3 not configured");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    let url = s3
        .upload(&key, &content_type, data.to_vec())
        .await
        .map_err(|e| {
            tracing::error!("S3 upload failed: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    tracing::info!(
        %hub_id, %channel_id, %attachment_id,
        name = %file_name, size = data.len(),
        "attachment uploaded"
    );

    Ok(Json(AttachmentResponse {
        attachment_id: attachment_id.to_string(),
        url,
        content_type,
        name: file_name,
        size: data.len(),
    }))
}
