use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

/// Well-known UUIDs for dev seed data.
/// Deterministic so they survive restarts.
pub const DEV_HUB_ID: Uuid = Uuid::from_bytes([
    0xDE, 0xF0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
]);

pub const DEV_USER_ALICE: Uuid = Uuid::from_bytes([
    0xDE, 0xF0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01,
]);

pub const DEV_USER_BOB: Uuid = Uuid::from_bytes([
    0xDE, 0xF0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02,
]);

pub const DEV_USER_CHARLIE: Uuid = Uuid::from_bytes([
    0xDE, 0xF0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x03,
]);

pub async fn run_dev_seed(pool: &PgPool) -> Result<()> {
    tracing::info!("running dev seed...");

    // Hub
    sqlx::query(
        "INSERT INTO hubs (id, name, slug, plan) VALUES ($1, $2, $3, $4)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(DEV_HUB_ID)
    .bind("Dev Hub")
    .bind("dev-hub")
    .bind("pro")
    .execute(pool)
    .await?;

    // Users
    let users = [
        (DEV_USER_ALICE, "alice", "Alice"),
        (DEV_USER_BOB, "bob", "Bob"),
        (DEV_USER_CHARLIE, "charlie", "Charlie"),
    ];
    for (id, username, display_name) in &users {
        sqlx::query(
            "INSERT INTO users (id, username, display_name) VALUES ($1, $2, $3)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(username)
        .bind(display_name)
        .execute(pool)
        .await?;
    }

    // Members
    let roles = [
        (DEV_USER_ALICE, "admin"),
        (DEV_USER_BOB, "member"),
        (DEV_USER_CHARLIE, "member"),
    ];
    for (user_id, role) in &roles {
        sqlx::query(
            "INSERT INTO hub_members (hub_id, user_id, role) VALUES ($1, $2, $3)
             ON CONFLICT (hub_id, user_id) DO NOTHING",
        )
        .bind(DEV_HUB_ID)
        .bind(user_id)
        .bind(role)
        .execute(pool)
        .await?;
    }

    // Channels
    let channels: &[(&str, &str, i32)] = &[
        ("general", "text", 0),
        ("random", "text", 1),
        ("voice-test", "voice", 2),
        ("stage-test", "stage", 3),
    ];
    for (name, ch_type, position) in channels {
        sqlx::query(
            "INSERT INTO channels (hub_id, name, type, position)
             SELECT $1, $2, $3, $4
             WHERE NOT EXISTS (
                 SELECT 1 FROM channels WHERE hub_id = $1 AND name = $2
             )",
        )
        .bind(DEV_HUB_ID)
        .bind(name)
        .bind(ch_type)
        .bind(position)
        .execute(pool)
        .await?;
    }

    tracing::info!(
        hub_id = %DEV_HUB_ID,
        "dev seed complete: hub 'Dev Hub', 3 users, 4 channels"
    );
    Ok(())
}
