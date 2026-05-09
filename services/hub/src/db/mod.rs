pub mod rls;
pub mod seed;

use anyhow::Result;
use matehub_common::snowflake;
use sqlx::{PgPool, postgres::PgPoolOptions};

/// Build the shared Postgres pool with a release-time RESET on
/// `app.current_hub_id`.
///
/// **Why this matters.** The RLS policies in `001_init.sql` make a single
/// load-bearing promise: a query without an `app.current_hub_id` set sees
/// no hub-scoped rows (because the policy predicate evaluates `NULL = ...`
/// → false). That promise is the cross-tenant isolation backstop the
/// whole product depends on.
///
/// Some helpers (`hub_connection`, `with_hub_context`, the seed path) bind
/// the GUC at **session** scope. When that connection comes back into the
/// pool, the binding survives. The next handler that fishes the connection
/// out — even one that "knows" it should rebind — has a window of bytes
/// where it inherits the previous tenant's view. A single missed rebind
/// downstream and an authenticated user reads another hub's rows through
/// a perfectly-named-and-pulled `SELECT * FROM channels`.
///
/// `after_release` runs unconditionally on the pool path that returns the
/// connection to the pool (including drop / panic unwind), so the GUC is
/// guaranteed cleared before the next caller can pick the connection up.
/// The cost is one round-trip per `release` — negligible against any real
/// query, and worth it to make the leak structurally impossible rather
/// than relying on every handler to remember.
pub async fn connect(database_url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .after_release(|conn, _meta| {
            Box::pin(async move {
                // RESET strips the binding regardless of how it was set
                // (`SET` or `SELECT set_config(..., false)`). It does
                // nothing if the GUC was never touched on this connection,
                // so the cost is bounded.
                sqlx::query("RESET app.current_hub_id")
                    .execute(&mut *conn)
                    .await?;
                Ok(true)
            })
        })
        .connect(database_url)
        .await?;
    tracing::info!("connected to database");
    Ok(pool)
}

const MIGRATIONS: &[(&str, &str)] = &[
    ("001_init", include_str!("../../migrations/001_init.sql")),
    (
        "002_invite_links",
        include_str!("../../migrations/002_invite_links.sql"),
    ),
];

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
