pub mod auth_api;
pub mod auth_check;
pub mod channels;
pub mod dev;
pub mod groups;
pub mod hubs;
pub mod members;
pub mod permissions;
pub mod presence_ws;
pub mod profile;
pub mod temp_users;

use std::sync::Arc;

use axum::Router;
use sqlx::PgPool;

use tokio::sync::broadcast;

use crate::api::channels::ChannelsState;
use crate::api::members::MembersState;
use crate::api::presence_ws::{PresenceEvent, PresenceState};
use crate::api::profile::ProfileState;
use crate::presence::RedisPool;
use crate::storage::S3Storage;

pub fn routes(
    pool: PgPool,
    s3: Option<Arc<S3Storage>>,
    redis: Option<RedisPool>,
    events: broadcast::Sender<PresenceEvent>,
    dev_mode: bool,
) -> Router {
    let channels_state = ChannelsState {
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
        .nest("/v1", auth_api::routes(pool.clone()))
        .nest("/v1", hubs::routes(pool.clone()))
        .nest("/v1", channels::routes(channels_state))
        .nest("/v1", groups::routes(pool.clone()))
        .nest("/v1", permissions::routes(pool.clone()))
        .nest("/v1", temp_users::routes(pool.clone()))
        .nest("/v1", profile::routes(profile_state))
        .nest("/v1", members::routes(members_state))
        .merge(presence_ws::routes(presence_state));

    if dev_mode {
        app = app.nest("/dev", dev::routes(pool));
    }

    app
}
