use anyhow::Result;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub struct OutboxRow {
    pub id: Uuid,
    pub template: String,
    pub to_email: String,
    pub payload: Value,
    pub attempts: i32,
}

/// Enqueue a mail within a caller-provided transaction. This is how callers
/// get transactional-outbox semantics: the mail insert commits or aborts
/// with the business insert that triggered it.
pub async fn enqueue_tx(
    tx: &mut Transaction<'_, Postgres>,
    template: &str,
    to_email: &str,
    payload: Value,
) -> Result<Uuid> {
    let id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO mail_outbox (template, to_email, payload)
        VALUES ($1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(template)
    .bind(to_email)
    .bind(payload)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

/// Claim up to `limit` pending rows atomically. Rows are marked 'sending'
/// so no other worker picks them up; returned for caller to send.
pub async fn claim_batch(pool: &PgPool, limit: i64) -> Result<Vec<OutboxRow>> {
    let rows: Vec<(Uuid, String, String, Value, i32)> = sqlx::query_as(
        r#"
        WITH claimed AS (
            SELECT id FROM mail_outbox
            WHERE status = 'pending' AND scheduled_at <= now()
            ORDER BY scheduled_at
            FOR UPDATE SKIP LOCKED
            LIMIT $1
        )
        UPDATE mail_outbox m
        SET status = 'sending', attempts = m.attempts + 1
        FROM claimed
        WHERE m.id = claimed.id
        RETURNING m.id, m.template, m.to_email, m.payload, m.attempts
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, template, to_email, payload, attempts)| OutboxRow {
            id,
            template,
            to_email,
            payload,
            attempts,
        })
        .collect())
}

pub async fn mark_sent(pool: &PgPool, id: Uuid, message_id: &str) -> Result<()> {
    sqlx::query(
        "UPDATE mail_outbox SET status = 'sent', sent_at = now(), message_id = $2 WHERE id = $1",
    )
    .bind(id)
    .bind(message_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_failed(pool: &PgPool, id: Uuid, err: &str, attempts: i32) -> Result<()> {
    // Retry schedule: 30s, 2m, 10m, 1h, 6h, then give up at attempt 6.
    let backoff_secs: i64 = match attempts {
        1 => 30,
        2 => 120,
        3 => 600,
        4 => 3600,
        5 => 6 * 3600,
        _ => {
            sqlx::query(
                "UPDATE mail_outbox SET status = 'failed', last_error = $2 WHERE id = $1",
            )
            .bind(id)
            .bind(err)
            .execute(pool)
            .await?;
            return Ok(());
        }
    };

    sqlx::query(
        r#"
        UPDATE mail_outbox
        SET status = 'pending',
            last_error = $2,
            scheduled_at = now() + ($3::bigint || ' seconds')::interval
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(err)
    .bind(backoff_secs)
    .execute(pool)
    .await?;
    Ok(())
}
