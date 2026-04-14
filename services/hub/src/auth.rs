use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// JWT claims -- shared across all services (video, chat, hub).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// User ID (permanent user) or temp_user ID
    pub sub: Uuid,
    /// Username / nickname
    pub username: String,
    /// "permanent" or "temp"
    pub user_type: String,
    /// Hub ID this token is scoped to
    pub hub_id: Uuid,
    /// Group IDs for permission checks
    pub groups: Vec<Uuid>,
    /// Issued at (unix timestamp)
    pub iat: i64,
    /// Expiry (unix timestamp)
    pub exp: i64,
}

/// Shared secret for signing/verifying JWTs.
/// In prod, this comes from env. All services in the троица share the same secret.
fn jwt_secret() -> Vec<u8> {
    std::env::var("JWT_SECRET")
        .unwrap_or_else(|_| "matehub-dev-secret-change-in-prod".into())
        .into_bytes()
}

pub fn create_token(claims: &Claims) -> Result<String, jsonwebtoken::errors::Error> {
    encode(
        &Header::default(),
        claims,
        &EncodingKey::from_secret(&jwt_secret()),
    )
}

pub fn verify_token(token: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(&jwt_secret()),
        &Validation::default(),
    )?;
    Ok(data.claims)
}

/// Extractor: pull Claims from Authorization header.
/// Usage: `async fn handler(claims: AuthUser, ...)`
/// Returns 401 if missing/invalid.
#[derive(Debug, Clone)]
pub struct AuthUser(pub Claims);

impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or(StatusCode::UNAUTHORIZED)?;

        let token = header
            .strip_prefix("Bearer ")
            .ok_or(StatusCode::UNAUTHORIZED)?;

        // Dev mode: accept "dev-{username}-token" format
        if token.starts_with("dev-") && token.ends_with("-token") {
            let username = token
                .strip_prefix("dev-")
                .and_then(|s| s.strip_suffix("-token"))
                .ok_or(StatusCode::UNAUTHORIZED)?;

            return Ok(AuthUser(Claims {
                sub: dev_user_id(username),
                username: username.to_string(),
                user_type: "permanent".into(),
                hub_id: crate::db::seed::DEV_HUB_ID,
                groups: vec![],
                iat: chrono::Utc::now().timestamp(),
                exp: chrono::Utc::now().timestamp() + 86400,
            }));
        }

        let claims = verify_token(token).map_err(|_| StatusCode::UNAUTHORIZED)?;
        Ok(AuthUser(claims))
    }
}

/// Map dev username to deterministic UUID
fn dev_user_id(username: &str) -> Uuid {
    match username {
        "alice" => crate::db::seed::DEV_USER_ALICE,
        "bob" => crate::db::seed::DEV_USER_BOB,
        "charlie" => crate::db::seed::DEV_USER_CHARLIE,
        _ => Uuid::new_v4(),
    }
}
