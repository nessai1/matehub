pub mod hubs;
pub mod dev;

use axum::Router;
use sqlx::PgPool;

pub fn routes(pool: PgPool, dev_mode: bool) -> Router {
    let mut app = Router::new()
        .nest("/v1", hubs::routes(pool.clone()));

    if dev_mode {
        app = app.nest("/dev", dev::routes(pool));
    }

    app
}
