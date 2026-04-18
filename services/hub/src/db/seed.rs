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

// Groups
const DEV_GROUP_EVERYONE: Uuid = Uuid::from_bytes([
    0xDE, 0xF0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x01,
]);

const DEV_GROUP_ADMIN: Uuid = Uuid::from_bytes([
    0xDE, 0xF0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x02,
]);

const DEV_GROUP_GUESTS: Uuid = Uuid::from_bytes([
    0xDE, 0xF0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x03,
]);

use crate::models::permission::bits;

const ALL_PERMS: i32 = bits::ALL;
const MEMBER_PERMS: i32 = bits::MEMBER_CHANNEL;

pub async fn run_dev_seed(pool: &PgPool) -> Result<()> {
    tracing::info!("running dev seed...");

    // Bypass RLS for seeding (superuser or set hub context)
    sqlx::raw_sql(&format!("SET LOCAL app.current_hub_id = '{}'", DEV_HUB_ID))
        .execute(pool)
        .await
        .ok(); // ignore if RLS not yet active

    // Users first (hub FK references creator_id -> users)
    let password_hash = bcrypt::hash("123123", bcrypt::DEFAULT_COST)?;
    let users = [
        (DEV_USER_ALICE, "alice", "Alice", "alice@matehub.dev"),
        (DEV_USER_BOB, "bob", "Bob", "bob@matehub.dev"),
        (DEV_USER_CHARLIE, "charlie", "Charlie", "charlie@matehub.dev"),
    ];
    for (id, username, display_name, email) in &users {
        sqlx::query(
            "INSERT INTO users (id, username, display_name, email, password_hash)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (id) DO UPDATE SET
                password_hash = COALESCE(users.password_hash, $5),
                email = COALESCE(users.email, $4)",
        )
        .bind(id)
        .bind(username)
        .bind(display_name)
        .bind(email)
        .bind(&password_hash)
        .execute(pool)
        .await?;
    }

    // Hub (after users, because creator_id FK)
    sqlx::query(
        "INSERT INTO hubs (id, name, slug, plan, creator_id) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (id) DO UPDATE SET creator_id = COALESCE(hubs.creator_id, $5)",
    )
    .bind(DEV_HUB_ID)
    .bind("Dev Hub")
    .bind("dev-hub")
    .bind("pro")
    .bind(DEV_USER_ALICE)
    .execute(pool)
    .await?;

    // Members (keep old table for backwards compat, role = group name)
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

    // ── Groups ──────────────────────────────────────
    // (id, name, color, position, is_default, hub_permissions)
    let groups = [
        (DEV_GROUP_EVERYONE, "everyone", "#99AAB5", 0, true, 0i32),
        (DEV_GROUP_ADMIN, "admin", "#E74C3C", 1, false, bits::ALL),
        (DEV_GROUP_GUESTS, "guests", "#95A5A6", 2, false, 0i32),
    ];
    for (id, name, color, position, is_default, hub_perms) in &groups {
        sqlx::query(
            "INSERT INTO groups (id, hub_id, name, color, position, is_default, hub_permissions)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (id) DO UPDATE SET hub_permissions = $7",
        )
        .bind(id)
        .bind(DEV_HUB_ID)
        .bind(name)
        .bind(color)
        .bind(position)
        .bind(is_default)
        .bind(hub_perms)
        .execute(pool)
        .await?;
    }

    // ── Member <-> Group assignments ────────────────
    let member_groups = [
        // Alice: everyone + admin
        (DEV_USER_ALICE, DEV_GROUP_EVERYONE),
        (DEV_USER_ALICE, DEV_GROUP_ADMIN),
        // Bob: everyone
        (DEV_USER_BOB, DEV_GROUP_EVERYONE),
        // Charlie: everyone
        (DEV_USER_CHARLIE, DEV_GROUP_EVERYONE),
    ];
    for (user_id, group_id) in &member_groups {
        sqlx::query(
            "INSERT INTO member_groups (hub_id, user_id, group_id) VALUES ($1, $2, $3)
             ON CONFLICT (hub_id, user_id, group_id) DO NOTHING",
        )
        .bind(DEV_HUB_ID)
        .bind(user_id)
        .bind(group_id)
        .execute(pool)
        .await?;
    }

    // ── Channels ────────────────────────────────────
    // "general" is created by ensure_default_channel (same as production)
    super::ensure_default_channel(pool, DEV_HUB_ID).await?;

    // Dev-only extra channels
    let extra_channels: &[(&str, &str, i32)] = &[
        ("random", "text", 1),
        ("voice-test", "voice", 2),
        ("stage-test", "stage", 3),
    ];
    for (name, ch_type, position) in extra_channels {
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

    // ── Channel permissions ─────────────────────────
    // Fetch channel IDs (they're auto-generated, not deterministic)
    let channel_rows: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, name FROM channels WHERE hub_id = $1")
            .bind(DEV_HUB_ID)
            .fetch_all(pool)
            .await?;

    for (ch_id, ch_name) in &channel_rows {
        // admin group: full access everywhere
        upsert_perm(pool, *ch_id, DEV_GROUP_ADMIN, ALL_PERMS, 0).await?;

        // everyone group: standard member access
        upsert_perm(pool, *ch_id, DEV_GROUP_EVERYONE, MEMBER_PERMS, 0).await?;

        // guests: read-only in text, connect+speak in voice, no access to stage
        match ch_name.as_str() {
            "general" | "random" => {
                upsert_perm(pool, *ch_id, DEV_GROUP_GUESTS, bits::READ, 0).await?;
            }
            "voice-test" => {
                upsert_perm(pool, *ch_id, DEV_GROUP_GUESTS, MEMBER_PERMS, 0).await?;
            }
            "stage-test" => {
                // guests have no access to stage -- no row = deny
            }
            _ => {}
        }
    }

    tracing::info!(
        hub_id = %DEV_HUB_ID,
        "dev seed complete: hub 'Dev Hub', 3 users, 3 groups, 4 channels with permissions"
    );
    Ok(())
}

async fn upsert_perm(
    pool: &PgPool,
    channel_id: Uuid,
    group_id: Uuid,
    allow: i32,
    deny: i32,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO channel_permissions (channel_id, group_id, allow_bits, deny_bits)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (channel_id, group_id)
         DO UPDATE SET allow_bits = $3, deny_bits = $4",
    )
    .bind(channel_id)
    .bind(group_id)
    .bind(allow)
    .bind(deny)
    .execute(pool)
    .await?;
    Ok(())
}
