// Pool isolation invariant.
//
// `db::connect` attaches an `after_release` callback that RESETs
// `app.current_hub_id` whenever a connection returns to the pool. That
// callback is the load-bearing piece keeping RLS honest under the
// `hub_connection` / `with_hub_context` pattern: those helpers bind the
// GUC at session scope, and without the post-release reset the binding
// would survive into the next caller's request — turning RLS from a
// defense-in-depth backstop into "works only while every handler is
// careful". This test pins the invariant so a future refactor doesn't
// silently drop the callback.

mod common;

use sqlx::PgPool;

#[tokio::test]
async fn pool_clears_app_current_hub_id_between_acquires() {
    // Use the production `db::connect` path. If someone ever removes the
    // after_release callback, this test fails — that's the entire point.
    common::spawn_app().await; // ensures matehub_test exists & migrations applied
    let url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://matehub:matehub-dev@localhost:5432/matehub_test".into());
    let pool: PgPool = matehub_hub::db::connect(&url).await.unwrap();

    // 20 iterations to rotate through whatever physical connections the
    // pool keeps. With default `max_connections=10`, this guarantees we
    // exercise each connection at least once across set + reacquire.
    // A single leaked binding on any connection trips the assertion.
    for i in 0..20 {
        // Phase 1: take a connection, plant a binding on it, drop it.
        {
            let mut conn = pool.acquire().await.unwrap();
            sqlx::query("SET app.current_hub_id = '999999'")
                .execute(&mut *conn)
                .await
                .unwrap();
            // Sanity: the binding actually took effect on this connection.
            // If this fails the test setup is broken, not the invariant.
            let observed: Option<String> =
                sqlx::query_scalar("SELECT current_setting('app.current_hub_id', true)")
                    .fetch_one(&mut *conn)
                    .await
                    .unwrap();
            assert_eq!(
                observed.as_deref(),
                Some("999999"),
                "iteration {i}: SET didn't take effect (test setup issue)"
            );
        } // drop → pool.release → after_release fires

        // Phase 2: take another connection from the pool. May or may not
        // be the same physical connection — the invariant we want is
        // global: *no* connection in the pool is allowed to come back
        // bound. `current_setting(name, missing_ok=true)` returns "" for
        // a freshly-RESET GUC (Postgres convention).
        let mut conn = pool.acquire().await.unwrap();
        let observed: Option<String> =
            sqlx::query_scalar("SELECT current_setting('app.current_hub_id', true)")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        let leaked = observed.as_deref().unwrap_or("");
        assert!(
            leaked.is_empty(),
            "iteration {i}: pool returned a connection with leaked app.current_hub_id={leaked:?}"
        );
    }
}
