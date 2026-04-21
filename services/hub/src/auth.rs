use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
};

pub use matehub_common::auth::{Claims, create_token, verify_token};

/// Extractor: pull Claims from the Authorization header.
/// Returns 401 if missing/invalid. In dev mode (see the `dev-{username}-token`
/// shortcut below) it synthesises a Claims directly so the frontend can skip
/// the full login dance.
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

        // Dev mode: accept "dev-{username}-token" format.
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

fn dev_user_id(username: &str) -> i64 {
    match username {
        "alice" => crate::db::seed::DEV_USER_ALICE,
        "bob" => crate::db::seed::DEV_USER_BOB,
        "charlie" => crate::db::seed::DEV_USER_CHARLIE,
        // Unknown dev user → salt from hash of the name. Caller still needs a
        // hub membership to get past the first real DB query.
        other => {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut h = DefaultHasher::new();
            other.hash(&mut h);
            (h.finish() & 0x7FFF_FFFF_FFFF_FFFF) as i64
        }
    }
}
