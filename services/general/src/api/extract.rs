use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;

use crate::auth::{AccountClaims, verify_account_token};
use crate::state::AppState;

/// Axum extractor for authenticated account-level requests.
/// Reads `Authorization: Bearer <token>` and validates against `GENERAL_JWT_SECRET`.
pub struct Authed(pub AccountClaims);

impl FromRequestParts<Arc<AppState>> for Authed {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .ok_or((StatusCode::UNAUTHORIZED, "missing Authorization"))?;

        let token = header
            .strip_prefix("Bearer ")
            .ok_or((StatusCode::UNAUTHORIZED, "expected Bearer token"))?;

        let claims = verify_account_token(token, &state.config.jwt_secret)
            .map_err(|_| (StatusCode::UNAUTHORIZED, "invalid token"))?;

        Ok(Authed(claims))
    }
}
