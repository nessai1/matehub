use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;

use super::outbox::{self, OutboxRow};
use super::templates;
use super::transport::MailTransport;

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const BATCH_SIZE: i64 = 32;

pub async fn run(pool: PgPool, transport: Arc<dyn MailTransport>, mail_from: String) {
    tracing::info!("mail outbox worker started");
    loop {
        match tick(&pool, transport.as_ref(), &mail_from).await {
            Ok(0) => tokio::time::sleep(POLL_INTERVAL).await,
            Ok(_) => {}
            Err(e) => {
                tracing::error!(error = %e, "outbox worker tick failed");
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
    }
}

async fn tick(
    pool: &PgPool,
    transport: &dyn MailTransport,
    mail_from: &str,
) -> anyhow::Result<usize> {
    let batch = outbox::claim_batch(pool, BATCH_SIZE).await?;
    let n = batch.len();
    for row in batch {
        process_row(pool, transport, mail_from, row).await?;
    }
    Ok(n)
}

async fn process_row(
    pool: &PgPool,
    transport: &dyn MailTransport,
    mail_from: &str,
    row: OutboxRow,
) -> anyhow::Result<()> {
    let mail = match templates::render(&row.template, &row.to_email, &row.payload, mail_from) {
        Ok(m) => m,
        Err(e) => {
            // Render failures are permanent — same input will never render
            // differently. Mark failed immediately, skip the backoff dance.
            tracing::error!(id = %row.id, error = %e, "mail render failed; marking failed");
            outbox::mark_failed(pool, row.id, &e.to_string(), 99).await?;
            return Ok(());
        }
    };

    match transport.send(&mail).await {
        Ok(message_id) => {
            outbox::mark_sent(pool, row.id, &message_id).await?;
        }
        Err(e) => {
            tracing::warn!(id = %row.id, error = %e, "mail send failed; will retry");
            outbox::mark_failed(pool, row.id, &e.to_string(), row.attempts).await?;
        }
    }
    Ok(())
}
