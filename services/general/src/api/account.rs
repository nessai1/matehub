use std::sync::Arc;

use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use serde::Serialize;

use crate::api::extract::Authed;
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/account", get(me))
}

#[derive(Serialize)]
struct AccountResponse {
    #[serde(with = "matehub_common::serde_i64::as_string")]
    id: i64,
    email: String,
    email_verified: bool,
}

async fn me(
    State(state): State<Arc<AppState>>,
    Authed(claims): Authed,
) -> Result<Json<AccountResponse>, (StatusCode, String)> {
    let row: Option<(i64, String, bool)> =
        sqlx::query_as("SELECT id, email, email_verified FROM accounts WHERE id = $1")
            .bind(claims.sub)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (id, email, email_verified) =
        row.ok_or((StatusCode::NOT_FOUND, "account not found".into()))?;

    Ok(Json(AccountResponse {
        id,
        email,
        email_verified,
    }))
}
