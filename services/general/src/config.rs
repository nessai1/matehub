use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub port: u16,
    pub jwt_secret: String,
    pub account_token_ttl_secs: i64,
    pub public_base_url: String,
    pub mail_from: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            database_url: std::env::var("DATABASE_URL").unwrap_or_else(|_| {
                "postgresql://matehub:matehub-dev@localhost:5432/matehub_general".into()
            }),
            port: std::env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3001),
            jwt_secret: std::env::var("GENERAL_JWT_SECRET")
                .context("GENERAL_JWT_SECRET must be set")?,
            account_token_ttl_secs: 24 * 3600,
            public_base_url: std::env::var("PUBLIC_BASE_URL")
                .unwrap_or_else(|_| "https://matehub.io".into()),
            mail_from: std::env::var("MAIL_FROM")
                .unwrap_or_else(|_| "MateHub <noreply@matehub.io>".into()),
        })
    }
}
