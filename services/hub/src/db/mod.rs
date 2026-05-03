pub mod rls;
pub mod seed;

use anyhow::Result;
use matehub_common::snowflake;
use sqlx::PgPool;

pub async fn connect(database_url: &str) -> Result<PgPool> {
    let pool = PgPool::connect(database_url).await?;
    tracing::info!("connected to database");
    Ok(pool)
}

const MIGRATIONS: &[(&str, &str)] = &[("001_init", include_str!("../../migrations/001_init.sql"))];

pub async fn migrate(pool: &PgPool) -> Result<()> {
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS _migrations (
            name TEXT PRIMARY KEY,
            applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
        )",
    )
    .execute(pool)
    .await?;

    for (name, sql) in MIGRATIONS {
        let applied: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM _migrations WHERE name = $1)")
                .bind(name)
                .fetch_one(pool)
                .await?;

        if applied {
            continue;
        }

        tracing::info!(migration = name, "applying migration");
        sqlx::raw_sql(sql).execute(pool).await?;

        sqlx::query("INSERT INTO _migrations (name) VALUES ($1)")
            .bind(name)
            .execute(pool)
            .await?;
    }

    tracing::info!("migrations up to date");
    Ok(())
}

/// Ensure a "general" text channel exists for a hub.
/// Called on hub creation (API + dev seed). Idempotent.
pub async fn ensure_default_channel(pool: &PgPool, hub_id: i64) -> Result<()> {
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM channels WHERE hub_id = $1 AND name = 'general' AND type = 'text'",
    )
    .bind(hub_id)
    .fetch_optional(pool)
    .await?;

    if existing.is_some() {
        return Ok(());
    }

    sqlx::query(
        "INSERT INTO channels (id, hub_id, name, type, position) VALUES ($1, $2, 'general', 'text', 0)",
    )
    .bind(snowflake::next_id())
    .bind(hub_id)
    .execute(pool)
    .await?;
    Ok(())
}
