use sqlx::{PgPool, Postgres, postgres::PgArguments};

/// Set RLS context (`app.current_hub_id`) for the duration of the
/// **transaction**. Cleared on commit/rollback automatically — the
/// connection that comes back to the pool has no leftover binding.
///
/// This is the right helper for every API handler that opens its own
/// `pool.begin()` and does hub-scoped INSERT/UPDATE/SELECT inside.
///
/// Equivalent to `SET LOCAL app.current_hub_id = '<hub_id>'`, but with
/// the value passed via bind so the format-bang shape can't be copy-
/// pasted with a request-body string downstream.
pub async fn set_hub_context_local<'c, E>(executor: E, hub_id: i64) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'c, Database = Postgres>,
{
    set_config(executor, hub_id, true).await
}

/// Set RLS context at the **session/connection** scope. The binding
/// persists past the next `COMMIT` and stays alive until the connection
/// is dropped — so this only belongs in the connection-handing helpers
/// below (`hub_connection`, `with_hub_context`) where the *caller* is
/// expected to drop the connection promptly. Inside an API handler that
/// returns the connection to the pool, prefer
/// [`set_hub_context_local`] — otherwise the next user of that pooled
/// connection inherits this hub's binding.
///
/// Equivalent to `SET app.current_hub_id = '<hub_id>'`.
pub async fn set_hub_context_session<'c, E>(executor: E, hub_id: i64) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'c, Database = Postgres>,
{
    set_config(executor, hub_id, false).await
}

async fn set_config<'c, E>(executor: E, hub_id: i64, is_local: bool) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'c, Database = Postgres>,
{
    let q: sqlx::query::Query<'_, Postgres, PgArguments> =
        sqlx::query("SELECT set_config('app.current_hub_id', $1, $2)")
            .bind(hub_id.to_string())
            .bind(is_local);
    q.execute(executor).await.map(|_| ())
}

/// Execute a closure within a transaction that has hub_id set for RLS.
/// Every query inside the closure is automatically filtered to this hub.
#[allow(dead_code)]
pub async fn with_hub_context<F, Fut, T>(pool: &PgPool, hub_id: i64, f: F) -> Result<T, sqlx::Error>
where
    F: FnOnce(sqlx::pool::PoolConnection<sqlx::Postgres>) -> Fut,
    Fut: std::future::Future<Output = Result<T, sqlx::Error>>,
{
    let mut conn = pool.acquire().await?;
    set_hub_context_session(&mut *conn, hub_id).await?;
    f(conn).await
}

/// Simpler version: just set the context and return the connection.
/// Caller is responsible for using it.
pub async fn hub_connection(
    pool: &PgPool,
    hub_id: i64,
) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    set_hub_context_session(&mut *conn, hub_id).await?;
    Ok(conn)
}
