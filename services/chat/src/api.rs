use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, patch, post},
};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::data_service::DataService;
use crate::fanout::FanoutService;
use crate::models::{Message, SendMessageRequest, events};
use crate::read_state::{self, RedisPool};
use crate::snowflake;

#[derive(Clone)]
pub struct AppState {
    pub data: Arc<DataService>,
    pub fanout: Arc<FanoutService>,
    pub redis: Option<RedisPool>,
    pub s3: Option<Arc<matehub_common::storage::S3Storage>>,
    pub sessions: crate::session::SessionStore,
}

pub fn routes(state: AppState) -> Router {
    Router::new()
        .route("/v1/channels/{channel_id}/messages", post(send_message).get(get_history))
        .route("/v1/channels/{channel_id}/messages/{message_id}", patch(edit_message).delete(delete_message))
        .route("/v1/channels/{channel_id}/typing", post(typing))
        .route("/v1/channels/{channel_id}/ack", post(mark_read))
        .route("/v1/sync", post(sync))
        .merge(crate::attachments::routes())
        .route("/health", get(|| async { "ok" }))
        .with_state(state)
}

// ── Send Message ────────────────────────────────

async fn send_message(
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<SendMessageRequest>,
) -> Result<(StatusCode, Json<Message>), StatusCode> {
    let hub_id = auth.0.hub_id.parse::<i64>().map_err(|_| StatusCode::BAD_REQUEST)?;

    if body.content.trim().is_empty() && body.attachments.as_ref().is_none_or(|a| a.is_empty()) {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Rate limit: 5 messages/5s per user per channel
    if let Some(mut redis) = state.redis.clone() {
        if !read_state::check_rate_limit(&mut redis, &auth.0.sub, channel_id, 5, 5).await {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
    }

    // Idempotency: if client_id was seen, return cached message_id
    if let (Some(client_id), Some(mut redis)) = (body.client_id.as_deref(), state.redis.clone()) {
        if let Some(existing_id) = read_state::check_idempotency(&mut redis, &auth.0.sub, client_id).await {
            tracing::debug!(client_id, existing_id, "idempotent duplicate");
            // Return minimal response -- client already has the message
            return Ok((StatusCode::OK, Json(Message {
                hub_id, channel_id, message_id: existing_id,
                author_id: auth.0.sub.clone(), author_type: auth.0.user_type.clone(),
                content: body.content.clone(), thread_root_id: body.thread_root_id,
                mentions: vec![], mention_groups: vec![], mention_everyone: false,
                attachments: vec![], edited_at: None, deleted_at: None,
                client_id: Some(client_id.to_string()), bucket: snowflake::current_bucket(),
            })));
        }
    }

    let mentions = parse_mentions(&body.content);

    let msg = state
        .data
        .write_message(
            hub_id, channel_id, &auth.0.sub, &auth.0.user_type,
            body.content.trim(), mentions, vec![],
            body.content.contains("@everyone"),
            body.attachments.unwrap_or_default(),
            body.thread_root_id, body.client_id.clone(),
        )
        .await
        .map_err(|e| {
            tracing::error!("write_message failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Store idempotency
    if let (Some(client_id), Some(mut redis)) = (body.client_id.as_deref(), state.redis.clone()) {
        read_state::set_idempotency(&mut redis, &auth.0.sub, client_id, msg.message_id).await;
    }

    // Fan out
    state.fanout.publish_message(&msg).await;

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
    let hub_id = auth.0.hub_id.parse::<i64>().map_err(|_| StatusCode::BAD_REQUEST)?;

    if body.content.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let bucket = snowflake::current_bucket(); // TODO: extract from message_id or lookup

    let mentions = parse_mentions(&body.content);

    state.data
        .edit_message(
            hub_id, channel_id, bucket, message_id,
            body.content.trim(), mentions, vec![],
            body.content.contains("@everyone"),
        )
        .await
        .map_err(|e| {
            tracing::error!("edit_message failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Fan out edit event
    state.fanout.publish_event(hub_id, channel_id, events::MESSAGE_UPDATE, &serde_json::json!({
        "message_id": message_id,
        "channel_id": channel_id,
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
    let hub_id = auth.0.hub_id.parse::<i64>().map_err(|_| StatusCode::BAD_REQUEST)?;
    let bucket = snowflake::current_bucket();

    state.data
        .delete_message(hub_id, channel_id, bucket, message_id)
        .await
        .map_err(|e| {
            tracing::error!("delete_message failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    state.fanout.publish_event(hub_id, channel_id, events::MESSAGE_DELETE, &serde_json::json!({
        "message_id": message_id,
        "channel_id": channel_id,
    })).await;

    Ok(StatusCode::NO_CONTENT)
}

// ── Typing Indicator ────────────────────────────

async fn typing(
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    auth: AuthUser,
) -> StatusCode {
    let hub_id = auth.0.hub_id.parse::<i64>().unwrap_or(0);
    state.fanout.publish_typing(hub_id, channel_id, &auth.0.username).await;
    StatusCode::NO_CONTENT
}

// ── Mark Read (ACK) ─────────────────────────────

#[derive(Deserialize)]
struct AckRequest {
    message_id: i64,
}

async fn mark_read(
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    auth: AuthUser,
    Json(_body): Json<AckRequest>,
) -> StatusCode {
    if let Some(mut redis) = state.redis.clone() {
        read_state::mark_read(&mut redis, &auth.0.sub, channel_id).await;
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
    let hub_id = auth.0.hub_id.parse::<i64>().map_err(|_| StatusCode::BAD_REQUEST)?;
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

// ── Sync (offline catch-up) ─────────────────────

#[derive(Deserialize)]
struct SyncRequest {
    channels: Vec<SyncChannelRequest>,
}

#[derive(Deserialize)]
struct SyncChannelRequest {
    channel_id: i64,
    after: i64, // last known message_id
}

#[derive(serde::Serialize)]
struct SyncResponse {
    channels: Vec<SyncChannelResponse>,
}

#[derive(serde::Serialize)]
struct SyncChannelResponse {
    channel_id: i64,
    messages: Vec<Message>,
    limited: bool,
}

async fn sync(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<SyncRequest>,
) -> Result<Json<SyncResponse>, StatusCode> {
    let hub_id = auth.0.hub_id.parse::<i64>().map_err(|_| StatusCode::BAD_REQUEST)?;

    if body.channels.len() > 50 {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut channels = Vec::with_capacity(body.channels.len());
    for ch in body.channels {
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

fn parse_mentions(content: &str) -> Vec<String> {
    content
        .split_whitespace()
        .filter(|w| w.starts_with('@') && w.len() > 1)
        .map(|w| w[1..].trim_end_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|s| !s.is_empty() && s != "everyone" && s != "here")
        .collect()
}
