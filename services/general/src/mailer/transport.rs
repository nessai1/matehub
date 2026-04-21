use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde::Serialize;

use super::Mail;

#[async_trait]
pub trait MailTransport: Send + Sync {
    async fn send(&self, mail: &Mail) -> Result<String>;
}

/// Builds a transport from env.
///
/// MATEHUB_MAIL_TRANSPORT in {resend, noop}. Default: noop.
/// For `resend`: RESEND_API_KEY must be set.
pub fn from_env() -> Arc<dyn MailTransport> {
    match std::env::var("MATEHUB_MAIL_TRANSPORT").as_deref() {
        Ok("resend") => {
            let api_key = std::env::var("RESEND_API_KEY")
                .expect("RESEND_API_KEY required when MATEHUB_MAIL_TRANSPORT=resend");
            Arc::new(ResendTransport::new(api_key))
        }
        _ => {
            tracing::warn!(
                "mail transport = noop; mails will be logged only. set MATEHUB_MAIL_TRANSPORT=resend"
            );
            Arc::new(NoopTransport)
        }
    }
}

// ── Resend HTTP API ───────────────────────────────────────────────────

pub struct ResendTransport {
    client: reqwest::Client,
    api_key: String,
}

impl ResendTransport {
    pub fn new(api_key: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
        }
    }
}

#[derive(Serialize)]
struct ResendRequest<'a> {
    from: &'a str,
    to: [&'a str; 1],
    subject: &'a str,
    html: &'a str,
    text: &'a str,
}

#[async_trait]
impl MailTransport for ResendTransport {
    async fn send(&self, mail: &Mail) -> Result<String> {
        let body = ResendRequest {
            from: &mail.from,
            to: [&mail.to],
            subject: &mail.subject,
            html: &mail.html,
            text: &mail.text,
        };

        let resp = self
            .client
            .post("https://api.resend.com/emails")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            anyhow::bail!("resend returned {status}: {txt}");
        }

        #[derive(serde::Deserialize)]
        struct R {
            id: String,
        }
        let r: R = resp.json().await?;
        Ok(r.id)
    }
}

// ── Noop (dev / no-mail deployments) ──────────────────────────────────

pub struct NoopTransport;

#[async_trait]
impl MailTransport for NoopTransport {
    async fn send(&self, mail: &Mail) -> Result<String> {
        tracing::info!(
            to = %mail.to,
            subject = %mail.subject,
            "noop mailer: would have sent mail"
        );
        Ok(format!("noop-{}", uuid::Uuid::new_v4()))
    }
}
