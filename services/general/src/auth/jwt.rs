use anyhow::Result;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

/// Account-level JWT. Scoped to the *user's account* on matehub.io, not to any
/// specific hub. In the SSO flow it's exchanged for a hub-scoped token at
/// `/api/hubs/:slug/token`, which hub signs with its own HUB_SECRET.
#[derive(Debug, Serialize, Deserialize)]
pub struct AccountClaims {
    pub sub: i64,
    pub email: String,
    pub exp: i64,
    pub iat: i64,
}

pub fn create_account_token(
    account_id: i64,
    email: &str,
    ttl_secs: i64,
    secret: &str,
) -> Result<String> {
    let now = chrono::Utc::now().timestamp();
    let claims = AccountClaims {
        sub: account_id,
        email: email.to_string(),
        iat: now,
        exp: now + ttl_secs,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;
    Ok(token)
}

pub fn verify_account_token(token: &str, secret: &str) -> Result<AccountClaims> {
    let data = decode::<AccountClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?;
    Ok(data.claims)
}
