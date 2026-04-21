use sqlx::PgPool;

/// Execute a closure within a transaction that has hub_id set for RLS.
/// Every query inside the closure is automatically filtered to this hub.
#[allow(dead_code)]
pub async fn with_hub_context<F, Fut, T>(
    pool: &PgPool,
    hub_id: i64,
    f: F,
) -> Result<T, sqlx::Error>
where
    F: FnOnce(sqlx::pool::PoolConnection<sqlx::Postgres>) -> Fut,
    Fut: std::future::Future<Output = Result<T, sqlx::Error>>,
{
    let mut conn = pool.acquire().await?;

    sqlx::query(&format!("SET app.current_hub_id = '{hub_id}'"))
        .execute(&mut *conn)
        .await?;

    f(conn).await
}

/// Simpler version: just set the context and return the connection.
/// Caller is responsible for using it.
pub async fn hub_connection(
    pool: &PgPool,
    hub_id: i64,
) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    sqlx::query(&format!("SET app.current_hub_id = '{hub_id}'"))
        .execute(&mut *conn)
        .await?;
    Ok(conn)
}
