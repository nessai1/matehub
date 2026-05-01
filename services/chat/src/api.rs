use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use serde::Deserialize;

use crate::access::{self, Action};
use crate::auth::AuthUser;
use crate::data_service::DataService;
use crate::fanout::FanoutService;
use crate::models::{Message, SendMessageRequest, events};
use crate::read_state::{self, RedisPool};
use matehub_common::snowflake;

#[derive(Clone)]
pub struct AppState {
    pub data: Arc<DataService>,
    pub fanout: Arc<FanoutService>,
    pub redis: Option<RedisPool>,
    pub s3: Option<Arc<matehub_common::storage::S3Storage>>,
    pub sessions: crate::session::SessionStore,
    /// Hub-service Postgres pool, shared read-only by access::check. Optional
    /// because in test/integration contexts the URL may not be configured;
    /// when None, access::check fails closed (denies everything).
    pub pg: Option<sqlx::PgPool>,
}

pub fn routes(state: AppState) -> Router {
    Router::new()
        .route("/v1/channels/{channel_id}/messages", post(send_message).get(get_history))
        .route("/v1/channels/{channel_id}/messages/{message_id}", patch(edit_message).delete(delete_message))
        .route("/v1/channels/{channel_id}/typing", post(typing))
        .route("/v1/channels/{channel_id}/ack", post(mark_read))
        .route("/v1/read-states", get(get_read_states))
        .route("/v1/sync", post(sync))
        .merge(crate::attachments::routes())
        .merge(crate::dm_calls::routes())
        .route("/health", get(|| async { "ok" }))
        .route("/version", get(version_handler))
        .with_state(state)
}

async fn version_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "service": "chat",
        "version": option_env!("MATEHUB_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
    }))
}

// ── Error envelope ──────────────────────────────
//
// Most send-path failures are plain status codes; 429 needs a `Retry-After`
// header so the client can show an honest countdown instead of guessing.
// Wrapping everything in this enum keeps the handler signature monomorphic
// without a custom Response builder at every error site.
enum SendError {
    Status(StatusCode),
    RateLimited(u64),
}

impl From<StatusCode> for SendError {
    fn from(s: StatusCode) -> Self {
        SendError::Status(s)
    }
}

impl IntoResponse for SendError {
    fn into_response(self) -> Response {
        match self {
            SendError::Status(s) => s.into_response(),
            SendError::RateLimited(secs) => {
                let mut resp = StatusCode::TOO_MANY_REQUESTS.into_response();
                if let Ok(value) = secs.to_string().parse() {
                    resp.headers_mut().insert(header::RETRY_AFTER, value);
                }
                resp
            }
        }
    }
}

// ── Send Message ────────────────────────────────

async fn send_message(
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<SendMessageRequest>,
) -> Result<(StatusCode, Json<Message>), SendError> {
    let hub_id = auth.0.hub_id;
    // Scylla/Redis keys still store user_id as text — stringify once at the boundary.
    let user_id = auth.0.sub.to_string();

    if !access::check(&state, hub_id, channel_id, auth.0.sub, Action::Write).await {
        return Err(StatusCode::FORBIDDEN.into());
    }

    if body.content.trim().is_empty() && body.attachments.as_ref().is_none_or(|a| a.is_empty()) {
        return Err(StatusCode::BAD_REQUEST.into());
    }

    // Rate limit: 30 messages / 10s per user per channel. Generous enough
    // for normal back-and-forth (incl. long pasted blocks broken into a
    // few sends), tight enough that scripts/loops trip it. The TTL of the
    // bucket flows back in `Retry-After` so the client can show a countdown.
    if let Some(mut redis) = state.redis.clone() {
        if let Err(retry_after) =
            read_state::check_rate_limit(&mut redis, &user_id, channel_id, 30, 10).await
        {
            return Err(SendError::RateLimited(retry_after));
        }
    }

    // Idempotency: if client_id was seen, return cached message_id
    if let (Some(client_id), Some(mut redis)) = (body.client_id.as_deref(), state.redis.clone()) {
        if let Some(existing_id) = read_state::check_idempotency(&mut redis, &user_id, client_id).await {
            tracing::debug!(client_id, existing_id, "idempotent duplicate");
            // Return minimal response -- client already has the message
            return Ok((StatusCode::OK, Json(Message {
                hub_id, channel_id, message_id: existing_id,
                author_id: user_id.clone(), author_type: auth.0.user_type.clone(),
                content: body.content.clone(), thread_root_id: body.thread_root_id,
                mentions: vec![], mention_groups: vec![], mention_everyone: false,
                attachments: vec![], edited_at: None, deleted_at: None,
                client_id: Some(client_id.to_string()), bucket: snowflake::current_bucket(),
            })));
        }
    }

    let content = sanitize_content(body.content.trim());
    let mentions = parse_mentions(&content);

    let msg = state
        .data
        .write_message(
            hub_id, channel_id, &user_id, &auth.0.user_type,
            &content, mentions, vec![],
            content.contains("@everyone"),
            body.attachments.unwrap_or_default(),
            body.thread_root_id, body.client_id.clone(),
        )
        .await
        .map_err(|e| {
            tracing::error!("write_message failed: {e}");
            SendError::Status(StatusCode::INTERNAL_SERVER_ERROR)
        })?;

    // Store idempotency
    if let (Some(client_id), Some(mut redis)) = (body.client_id.as_deref(), state.redis.clone()) {
        read_state::set_idempotency(&mut redis, &user_id, client_id, msg.message_id).await;
    }

    // Index attachments for the streaming proxy. Done here (not in upload)
    // because only now do we know which message_id/bucket they belong to;
    // aborted sends (no POST /messages) leave no index entry to garbage-collect.
    for att in &msg.attachments {
        if att.id.is_empty() {
            continue; // legacy bare-URL attachment — no id to key on
        }
        if let Err(e) = state
            .data
            .insert_attachment_index_row(
                &att.id,
                msg.hub_id,
                msg.channel_id,
                msg.message_id,
                msg.bucket,
                &att.url,
                &att.content_type,
                att.size as i64,
            )
            .await
        {
            tracing::error!(id = %att.id, "attachment index insert failed: {e}");
        }
    }

    // Fan out
    state.fanout.publish_message(&msg).await;

    // Enqueue transcode requests for any attachments needing conversion
    for att in &msg.attachments {
        if att.status == crate::attachment::AttachmentStatus::Transcoding {
            // Derive S3 key from URL
            let source_key = match build_s3_key(&att.url) {
                Some(k) => k,
                None => {
                    tracing::warn!(id = %att.id, url = %att.url, "cannot derive s3 key, skip transcode");
                    continue;
                }
            };
            let req = crate::transcode::make_request(
                att,
                source_key,
                msg.hub_id,
                msg.channel_id,
                msg.message_id,
                msg.bucket,
            );
            if let Err(e) = crate::transcode::publish_request(&state.fanout.nats, &req).await {
                tracing::error!(id = %att.id, "failed to enqueue transcode: {e}");
            }
        }
    }

    tracing::debug!(%hub_id, %channel_id, msg_id = msg.message_id, author = %auth.0.username, "message sent");

    Ok((StatusCode::CREATED, Json(msg)))
}

// ── Edit Message ────────────────────────────────

#[derive(Deserialize)]
struct EditRequest {
    content: String,
}

async fn edit_message(
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(body): Json<EditRequest>,
) -> Result<StatusCode, StatusCode> {
    let hub_id = auth.0.hub_id;

    if !access::check(&state, hub_id, channel_id, auth.0.sub, Action::Write).await {
        return Err(StatusCode::FORBIDDEN);
    }

    let content = sanitize_content(body.content.trim());

    if content.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let bucket = snowflake::bucket_from_id(message_id);

    let mentions = parse_mentions(&content);

    state.data
        .edit_message(
            hub_id, channel_id, bucket, message_id,
            &content, mentions, vec![],
            content.contains("@everyone"),
        )
        .await
        .map_err(|e| {
            tracing::error!("edit_message failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Fan out edit event. IDs as strings — see `Message` docstring for why.
    state.fanout.publish_event(hub_id, channel_id, events::MESSAGE_UPDATE, &serde_json::json!({
        "message_id": message_id.to_string(),
        "channel_id": channel_id.to_string(),
        "content": body.content.trim(),
        "edited": true,
    })).await;

    Ok(StatusCode::OK)
}

// ── Delete Message ──────────────────────────────

async fn delete_message(
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, StatusCode> {
    let hub_id = auth.0.hub_id;

    if !access::check(&state, hub_id, channel_id, auth.0.sub, Action::Write).await {
        return Err(StatusCode::FORBIDDEN);
    }

    let bucket = snowflake::bucket_from_id(message_id);

    state.data
        .delete_message(hub_id, channel_id, bucket, message_id)
        .await
        .map_err(|e| {
            tracing::error!("delete_message failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    state.fanout.publish_event(hub_id, channel_id, events::MESSAGE_DELETE, &serde_json::json!({
        "message_id": message_id.to_string(),
        "channel_id": channel_id.to_string(),
    })).await;

    Ok(StatusCode::NO_CONTENT)
}

// ── Typing Indicator ────────────────────────────

async fn typing(
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    auth: AuthUser,
) -> StatusCode {
    let hub_id = auth.0.hub_id;

    if !access::check(&state, hub_id, channel_id, auth.0.sub, Action::Write).await {
        return StatusCode::FORBIDDEN;
    }

    let user_id = auth.0.sub.to_string();
    state.fanout.publish_typing(hub_id, channel_id, &user_id).await;
    StatusCode::NO_CONTENT
}

// ── Mark Read (ACK) ─────────────────────────────

#[derive(Deserialize)]
struct AckRequest {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    message_id: i64,
}

async fn mark_read(
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<AckRequest>,
) -> StatusCode {
    let hub_id = auth.0.hub_id;

    if !access::check(&state, hub_id, channel_id, auth.0.sub, Action::Read).await {
        return StatusCode::FORBIDDEN;
    }

    let user_id = auth.0.sub.to_string();
    if let Some(mut redis) = state.redis.clone() {
        read_state::mark_read(
            &mut redis, &state.data,
            &user_id, hub_id, channel_id, body.message_id,
        ).await;
    } else {
        // No Redis -- write directly to ScyllaDB
        if let Err(e) = state.data.mark_read(&user_id, hub_id, channel_id, body.message_id).await {
            tracing::error!("mark_read failed: {e}");
        }
    }
    StatusCode::NO_CONTENT
}

// ── History ─────────────────────────────────────

#[derive(Deserialize)]
struct HistoryQuery {
    #[serde(default = "default_limit")]
    limit: i32,
    before: Option<i64>,
}

fn default_limit() -> i32 { 50 }

async fn get_history(
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    auth: AuthUser,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<Message>>, StatusCode> {
    let hub_id = auth.0.hub_id;

    if !access::check(&state, hub_id, channel_id, auth.0.sub, Action::Read).await {
        return Err(StatusCode::FORBIDDEN);
    }

    let limit = query.limit.clamp(1, 100);

    let messages = state.data
        .read_history(hub_id, channel_id, limit, query.before)
        .await
        .map_err(|e| {
            tracing::error!("read_history failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(messages))
}

// ── Read States (bulk fetch for sidebar badges) ─────

#[derive(serde::Serialize)]
struct ReadStateItem {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    channel_id: i64,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    last_read_message_id: i64,
    mention_count: i32,
}

async fn get_read_states(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<ReadStateItem>>, StatusCode> {
    let hub_id = auth.0.hub_id;
    let user_id = auth.0.sub.to_string();

    let rows = state
        .data
        .get_all_read_states(&user_id)
        .await
        .map_err(|e| {
            tracing::error!("get_all_read_states failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let items: Vec<ReadStateItem> = rows
        .into_iter()
        .filter(|(h, _, _, _)| *h == hub_id)
        .map(|(_, channel_id, last_read_message_id, mention_count)| ReadStateItem {
            channel_id,
            last_read_message_id,
            mention_count,
        })
        .collect();

    Ok(Json(items))
}

// ── Sync (offline catch-up) ─────────────────────

#[derive(Deserialize)]
struct SyncRequest {
    channels: Vec<SyncChannelRequest>,
}

#[derive(Deserialize)]
struct SyncChannelRequest {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    channel_id: i64,
    #[serde(with = "matehub_common::serde_i64::as_string")]
    after: i64, // last known message_id
}

#[derive(serde::Serialize)]
struct SyncResponse {
    channels: Vec<SyncChannelResponse>,
}

#[derive(serde::Serialize)]
struct SyncChannelResponse {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    channel_id: i64,
    messages: Vec<Message>,
    limited: bool,
}

async fn sync(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<SyncRequest>,
) -> Result<Json<SyncResponse>, StatusCode> {
    let hub_id = auth.0.hub_id;

    if body.channels.len() > 50 {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut channels = Vec::with_capacity(body.channels.len());
    for ch in body.channels {
        // Per-channel ACL check. A user syncing across many channels at once
        // can include some they no longer have access to (e.g., kicked from a
        // text channel after their tab went idle) — silently skip those
        // rather than 403-ing the whole batch.
        if !access::check(&state, hub_id, ch.channel_id, auth.0.sub, Action::Read).await {
            continue;
        }

        let (messages, limited) = state
            .data
            .read_since(hub_id, ch.channel_id, ch.after)
            .await
            .map_err(|e| {
                tracing::error!("sync read_since failed: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

        channels.push(SyncChannelResponse {
            channel_id: ch.channel_id,
            messages,
            limited,
        });
    }

    Ok(Json(SyncResponse { channels }))
}

/// Collapse 3+ consecutive newlines into 2. Prevents chat spam with empty lines.
/// Extract S3 key from a URL like `{endpoint}/{bucket}/{key}`.
/// Returns the key (path after bucket) or None if URL shape is unexpected.
fn build_s3_key(url: &str) -> Option<String> {
    // Strip protocol
    let without_proto = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    // Skip host+bucket by taking everything after the 2nd slash
    let mut parts = without_proto.splitn(3, '/');
    parts.next()?; // host
    parts.next()?; // bucket
    parts.next().map(|s| s.to_string())
}

fn sanitize_content(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let mut consecutive_newlines = 0u32;
    for ch in content.chars() {
        if ch == '\n' {
            consecutive_newlines += 1;
            if consecutive_newlines <= 2 {
                result.push(ch);
            }
        } else {
            consecutive_newlines = 0;
            result.push(ch);
        }
    }
    result
}

fn parse_mentions(content: &str) -> Vec<String> {
    content
        .split_whitespace()
        .filter(|w| w.starts_with('@') && w.len() > 1)
        .map(|w| w[1..].trim_end_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|s| !s.is_empty() && s != "everyone" && s != "here")
        .collect()
}
