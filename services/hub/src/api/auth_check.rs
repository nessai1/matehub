use sqlx::PgPool;

use crate::models::permission::bits;

/// User's resolved hub-level permissions + position info.
pub struct UserPerms {
    pub hub_bits: i32,
    pub top_position: i32,
    pub is_admin: bool,
    pub is_creator: bool,
}

impl UserPerms {
    pub fn has(&self, bit: i32) -> bool {
        self.is_admin || (self.hub_bits & bit == bit)
    }

    pub fn can_manage_position(&self, target_position: i32) -> bool {
        self.is_admin || self.top_position < target_position
    }

    pub fn grantable_bits(&self) -> i32 {
        if self.is_admin {
            bits::ALL
        } else {
            self.hub_bits
        }
    }
}

pub async fn resolve_user_perms(
    pool: &PgPool,
    hub_id: i64,
    user_id: i64,
) -> Result<UserPerms, sqlx::Error> {
    #[derive(sqlx::FromRow)]
    struct Row {
        name: String,
        position: i32,
        hub_permissions: i32,
    }

    let rows = sqlx::query_as::<_, Row>(
        "SELECT g.name, g.position, g.hub_permissions
         FROM groups g
         JOIN member_groups mg ON mg.group_id = g.id AND mg.hub_id = g.hub_id
         WHERE mg.hub_id = $1 AND mg.user_id = $2",
    )
    .bind(hub_id)
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    let hub_bits = rows.iter().fold(0, |acc, r| acc | r.hub_permissions);
    let top_position = rows.iter().map(|r| r.position).min().unwrap_or(i32::MAX);
    let is_admin = rows.iter().any(|r| r.name == "admin");

    let is_creator: bool = sqlx::query_scalar(
        "SELECT COALESCE(creator_id = $2, false) FROM hubs WHERE id = $1",
    )
    .bind(hub_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .unwrap_or(false);

    Ok(UserPerms {
        hub_bits,
        top_position,
        is_admin,
        is_creator,
    })
}

pub async fn resolve_target_position(
    pool: &PgPool,
    hub_id: i64,
    target_user_id: i64,
) -> Result<i32, sqlx::Error> {
    let pos: Option<i32> = sqlx::query_scalar(
        "SELECT MIN(g.position)
         FROM groups g
         JOIN member_groups mg ON mg.group_id = g.id AND mg.hub_id = g.hub_id
         WHERE mg.hub_id = $1 AND mg.user_id = $2",
    )
    .bind(hub_id)
    .bind(target_user_id)
    .fetch_one(pool)
    .await?;

    Ok(pos.unwrap_or(i32::MAX))
}
