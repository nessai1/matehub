use std::sync::Arc;

use axum::{Router, extract::State, http::StatusCode, routing::post};

use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/webhooks/stripe", post(stripe))
}

async fn stripe(State(_state): State<Arc<AppState>>) -> Result<StatusCode, (StatusCode, String)> {
    // TODO: verify Stripe signature header, dispatch on event type
    // (customer.subscription.created, invoice.payment_failed, etc.)
    Ok(StatusCode::NOT_IMPLEMENTED)
}
