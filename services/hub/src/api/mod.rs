pub mod auth_api;
pub mod channels;
pub mod config;
pub mod dev;
pub mod dms;
pub mod groups;
pub mod hubs;
pub mod invitations;
pub mod invite_links;
pub mod members;
pub mod permissions;
pub mod presence_ws;
pub mod profile;
pub mod services;
pub mod setup;
pub mod sso;
pub mod temp_users;

use std::sync::Arc;

use axum::{Json, Router, routing::get};
use serde_json::json;
use sqlx::PgPool;

use tokio::sync::broadcast;

use crate::api::channels::ChannelsState;
use crate::api::hubs::HubsState;
use crate::api::members::MembersState;
use crate::api::presence_ws::{PresenceEvent, PresenceState};
use crate::api::profile::ProfileState;
use crate::api::sso::SsoState;
use crate::presence::RedisPool;
use crate::storage::S3Storage;

pub fn routes(
    pool: PgPool,
    s3: Option<Arc<S3Storage>>,
    redis: Option<RedisPool>,
    events: broadcast::Sender<PresenceEvent>,
    dev_mode: bool,
    sso_state: SsoState,
) -> Router {
    let channels_state = ChannelsState {
        pool: pool.clone(),
        storage: s3.clone(),
    };
    let hubs_state = HubsState {
        pool: pool.clone(),
        storage: s3.clone(),
    };
    let profile_state = ProfileState {
        storage: s3,
        pool: pool.clone(),
    };
    let members_state = MembersState {
        pool: pool.clone(),
        redis: redis.clone(),
    };
    let presence_state = PresenceState {
        pool: pool.clone(),
        redis,
        events,
    };

    let mut app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/version", get(version_handler))
        .merge(config::routes())
        .merge(setup::routes(pool.clone()))
        .merge(invitations::routes(pool.clone()))
        .merge(invite_links::routes(pool.clone()))
        .nest("/v1", auth_api::routes(pool.clone()))
        .nest("/v1", hubs::routes(hubs_state))
        .nest("/v1", channels::routes(channels_state))
        .nest("/v1", groups::routes(pool.clone()))
        .nest("/v1", permissions::routes(pool.clone()))
        .nest("/v1", temp_users::routes(pool.clone()))
        .nest("/v1", profile::routes(profile_state))
        .nest("/v1", members::routes(members_state))
        .nest("/v1", dms::routes(pool.clone()))
        .nest("/v1", services::routes())
        .merge(sso::routes(sso_state))
        .merge(presence_ws::routes(presence_state));

    if dev_mode {
        app = app.nest("/dev", dev::routes(pool));
    }

    app
}

async fn version_handler() -> Json<serde_json::Value> {
    Json(json!({
        "service": "hub",
        "version": option_env!("MATEHUB_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
    }))
}
