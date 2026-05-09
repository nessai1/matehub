use anyhow::Result;
use sqlx::PgPool;

/// Reserved IDs for dev seed data. Intentionally far below the Snowflake ID
/// space (which starts at ~4.5 × 10¹⁴), so there's no way a runtime-generated
/// ID can ever collide with these.
pub const DEV_HUB_ID: i64 = 1;

pub const DEV_USER_ALICE: i64 = 1001;
pub const DEV_USER_BOB: i64 = 1002;
pub const DEV_USER_CHARLIE: i64 = 1003;

const DEV_GROUP_EVERYONE: i64 = 2001;
const DEV_GROUP_ADMIN: i64 = 2002;
const DEV_GROUP_GUESTS: i64 = 2003;

use matehub_common::perms::bits;

const ALL_PERMS: i32 = bits::ALL;
const MEMBER_PERMS: i32 = bits::MEMBER_CHANNEL;

pub async fn run_dev_seed(pool: &PgPool) -> Result<()> {
    tracing::info!("running dev seed...");

    // Bypass RLS for seeding. We bind at session scope on the pool — seed
    // runs once at startup before any user traffic, so the leak that
    // worries us in handler paths (a returned connection still carrying
    // this hub_id) doesn't apply here.
    crate::db::rls::set_hub_context_session(pool, DEV_HUB_ID)
        .await
        .ok();

    // Users first (hub FK references creator_id -> users)
    let password_hash = bcrypt::hash("123123", bcrypt::DEFAULT_COST)?;
    let users = [
        (DEV_USER_ALICE, "alice", "Alice", "alice@matehub.dev"),
        (DEV_USER_BOB, "bob", "Bob", "bob@matehub.dev"),
        (
            DEV_USER_CHARLIE,
            "charlie",
            "Charlie",
            "charlie@matehub.dev",
        ),
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

    // ── Groups ──────────────────────────────────────
    let groups = [
        (DEV_GROUP_ADMIN, "admin", "#E74C3C", 0, false, bits::ALL),
        (DEV_GROUP_GUESTS, "guests", "#95A5A6", 1, false, 0i32),
        (DEV_GROUP_EVERYONE, "everyone", "#99AAB5", 100, true, 0i32),
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
        (DEV_USER_ALICE, DEV_GROUP_EVERYONE),
        (DEV_USER_ALICE, DEV_GROUP_ADMIN),
        (DEV_USER_BOB, DEV_GROUP_EVERYONE),
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
    super::ensure_default_channel(pool, DEV_HUB_ID).await?;

    let extra_channels: &[(&str, &str, i32)] = &[
        ("random", "text", 1),
        ("voice-test", "voice", 2),
        ("stage-test", "stage", 3),
    ];
    for (name, ch_type, position) in extra_channels {
        let existing: Option<i64> =
            sqlx::query_scalar("SELECT id FROM channels WHERE hub_id = $1 AND name = $2")
                .bind(DEV_HUB_ID)
                .bind(name)
                .fetch_optional(pool)
                .await?;
        if existing.is_some() {
            continue;
        }

        sqlx::query(
            "INSERT INTO channels (id, hub_id, name, type, position)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(matehub_common::snowflake::next_id())
        .bind(DEV_HUB_ID)
        .bind(name)
        .bind(ch_type)
        .bind(position)
        .execute(pool)
        .await?;
    }

    // ── Channel permissions ─────────────────────────
    let channel_rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, name FROM channels WHERE hub_id = $1")
            .bind(DEV_HUB_ID)
            .fetch_all(pool)
            .await?;

    for (ch_id, ch_name) in &channel_rows {
        upsert_perm(pool, *ch_id, DEV_GROUP_ADMIN, ALL_PERMS, 0).await?;
        upsert_perm(pool, *ch_id, DEV_GROUP_EVERYONE, MEMBER_PERMS, 0).await?;

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
        hub_id = DEV_HUB_ID,
        "dev seed complete: hub 'Dev Hub', 3 users, 3 groups, 4 channels with permissions"
    );
    Ok(())
}

async fn upsert_perm(
    pool: &PgPool,
    channel_id: i64,
    group_id: i64,
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
