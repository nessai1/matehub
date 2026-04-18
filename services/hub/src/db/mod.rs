pub mod rls;
pub mod seed;

use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

pub async fn connect(database_url: &str) -> Result<PgPool> {
    let pool = PgPool::connect(database_url).await?;
    tracing::info!("connected to database");
    Ok(pool)
}

const MIGRATIONS: &[(&str, &str)] = &[
    ("001_init", include_str!("../../migrations/001_init.sql")),
    (
        "002_groups_permissions_temp_users",
        include_str!("../../migrations/002_groups_permissions_temp_users.sql"),
    ),
    (
        "003_presence",
        include_str!("../../migrations/003_presence.sql"),
    ),
    (
        "004_user_password",
        include_str!("../../migrations/004_user_password.sql"),
    ),
    (
        "005_user_email",
        include_str!("../../migrations/005_user_email.sql"),
    ),
    (
        "006_refresh_tokens",
        include_str!("../../migrations/006_refresh_tokens.sql"),
    ),
    (
        "007_channel_icon",
        include_str!("../../migrations/007_channel_icon.sql"),
    ),
    (
        "008_hub_permissions",
        include_str!("../../migrations/008_hub_permissions.sql"),
    ),
];

pub async fn migrate(pool: &PgPool) -> Result<()> {
    // Create migration tracking table
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS _migrations (
            name TEXT PRIMARY KEY,
            applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
        )",
    )
    .execute(pool)
    .await?;

    for (name, sql) in MIGRATIONS {
        let applied: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM _migrations WHERE name = $1)",
        )
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
pub async fn ensure_default_channel(pool: &PgPool, hub_id: Uuid) -> Result<()> {
    sqlx::query(
        "INSERT INTO channels (hub_id, name, type, position)
         SELECT $1, 'general', 'text', 0
         WHERE NOT EXISTS (
             SELECT 1 FROM channels WHERE hub_id = $1 AND name = 'general' AND type = 'text'
         )",
    )
    .bind(hub_id)
    .execute(pool)
    .await?;
    Ok(())
}
