pub mod auth_api;
pub mod dev;
pub mod groups;
pub mod hubs;
pub mod permissions;
pub mod profile;
pub mod temp_users;

use std::sync::Arc;

use axum::Router;
use sqlx::PgPool;

use crate::api::profile::ProfileState;
use crate::storage::S3Storage;

pub fn routes(pool: PgPool, s3: Option<Arc<S3Storage>>, dev_mode: bool) -> Router {
    let profile_state = ProfileState {
        storage: s3,
        pool: pool.clone(),
    };

    let mut app = Router::new()
        .nest("/v1", auth_api::routes(pool.clone()))
        .nest("/v1", hubs::routes(pool.clone()))
        .nest("/v1", groups::routes(pool.clone()))
        .nest("/v1", permissions::routes(pool.clone()))
        .nest("/v1", temp_users::routes(pool.clone()))
        .nest("/v1", profile::routes(profile_state));

    if dev_mode {
        app = app.nest("/dev", dev::routes(pool));
    }

    app
}
