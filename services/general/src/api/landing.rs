use std::sync::Arc;

use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde::Deserialize;

use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/landing/contact", post(contact))
        .route("/api/landing/early-access", post(early_access))
}

#[derive(Deserialize)]
#[allow(dead_code)] // fields validate the wire shape; TODO consumes them
struct ContactRequest {
    email: String,
    message: String,
}

async fn contact(
    State(_state): State<Arc<AppState>>,
    Json(_req): Json<ContactRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    // TODO: enqueue "new contact form submission" email to ops, store to DB
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
#[allow(dead_code)] // fields validate the wire shape; TODO consumes them
struct EarlyAccessRequest {
    email: String,
}

async fn early_access(
    State(_state): State<Arc<AppState>>,
    Json(_req): Json<EarlyAccessRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    // TODO: insert into early_access_waitlist, enqueue confirmation mail
    Ok(StatusCode::ACCEPTED)
}
