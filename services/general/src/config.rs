use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub port: u16,
    pub jwt_secret: String,
    pub account_token_ttl_secs: i64,
    pub public_base_url: String,
    pub mail_from: String,
    /// Template for SSO redirect URLs, with `{slug}` and `{code}` placeholders.
    /// Prod: `https://{slug}.matehub.io/auth/sso?code={code}`
    /// Dev (single-port):  `http://localhost:3002/hub/{slug}/auth/sso?code={code}`
    pub hub_sso_url_template: String,
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
            hub_sso_url_template: std::env::var("HUB_SSO_URL_TEMPLATE")
                .unwrap_or_else(|_| "https://{slug}.matehub.io/auth/sso?code={code}".into()),
        })
    }

    /// Render the configured SSO-redirect template with a specific hub slug
    /// and single-use code. Kept as a method (not a formatter) so the
    /// template stays an infrastructure concern, not a caller-level detail.
    pub fn hub_sso_url(&self, slug: &str, code: &str) -> String {
        self.hub_sso_url_template
            .replace("{slug}", slug)
            .replace("{code}", code)
    }
}
