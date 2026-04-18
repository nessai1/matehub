use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use scylla::client::session::Session;
use scylla::statement::prepared::PreparedStatement;

use crate::models::Message;
use crate::snowflake;

/// Data Service: persistence layer between business logic and ScyllaDB.
/// Encapsulates token-aware routing, prepared statements, bucket management.
/// Key lesson from Discord: this layer protects ScyllaDB from thundering herd.
/// Max events per channel in /sync before returning limited=true
const SYNC_LIMIT: usize = 300;
/// When limited, return only this many latest messages
const SYNC_LIMITED_RETURN: usize = 50;

pub struct DataService {
    session: Arc<Session>,
    insert_msg: PreparedStatement,
    select_history: PreparedStatement,
    select_since: PreparedStatement,
    insert_bucket: PreparedStatement,
    select_buckets: PreparedStatement,
    update_content: PreparedStatement,
    soft_delete: PreparedStatement,
    // Read state (source of truth in ScyllaDB, cached in Redis)
    upsert_read_state: PreparedStatement,
    select_read_state: PreparedStatement,
    select_read_states_for_user: PreparedStatement,
}

impl DataService {
    pub async fn new(session: Arc<Session>) -> Result<Self> {
        let insert_msg = session
            .prepare(
                "INSERT INTO messages (hub_id, channel_id, bucket, message_id, author_id, author_type, content, thread_root_id, mentions, mention_groups, mention_everyone, attachments, client_id)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .await?;

        let select_history = session
            .prepare(
                "SELECT hub_id, channel_id, bucket, message_id, author_id, author_type, content, thread_root_id, mentions, mention_groups, mention_everyone, attachments, edited_at, deleted_at, client_id
                 FROM messages
                 WHERE hub_id = ? AND channel_id = ? AND bucket = ?
                 ORDER BY message_id DESC
                 LIMIT ?",
            )
            .await?;

        let select_since = session
            .prepare(
                "SELECT hub_id, channel_id, bucket, message_id, author_id, author_type, content, thread_root_id, mentions, mention_groups, mention_everyone, attachments, edited_at, deleted_at, client_id
                 FROM messages
                 WHERE hub_id = ? AND channel_id = ? AND bucket = ? AND message_id > ?
                 ORDER BY message_id ASC
                 LIMIT ?",
            )
            .await?;

        let insert_bucket = session
            .prepare(
                "INSERT INTO channel_buckets (hub_id, channel_id, bucket) VALUES (?, ?, ?)",
            )
            .await?;

        let select_buckets = session
            .prepare(
                "SELECT bucket FROM channel_buckets WHERE hub_id = ? AND channel_id = ? ORDER BY bucket DESC LIMIT ?",
            )
            .await?;

        // Read state prepared statements
        let upsert_read_state = session
            .prepare(
                "INSERT INTO read_state (user_id, hub_id, channel_id, last_read_message_id, mention_count, updated_at)
                 VALUES (?, ?, ?, ?, 0, toTimestamp(now()))",
            )
            .await?;

        let select_read_state = session
            .prepare(
                "SELECT last_read_message_id, mention_count FROM read_state
                 WHERE user_id = ? AND hub_id = ? AND channel_id = ?",
            )
            .await?;

        let select_read_states_for_user = session
            .prepare(
                "SELECT hub_id, channel_id, last_read_message_id, mention_count FROM read_state
                 WHERE user_id = ?",
            )
            .await?;

        tracing::info!("DataService: prepared statements cached");

        let update_content = session
            .prepare(
                "UPDATE messages SET content = ?, edited_at = toTimestamp(now()), mentions = ?, mention_groups = ?, mention_everyone = ?
                 WHERE hub_id = ? AND channel_id = ? AND bucket = ? AND message_id = ?",
            )
            .await?;

        let soft_delete = session
            .prepare(
                "UPDATE messages SET deleted_at = toTimestamp(now()), content = ''
                 WHERE hub_id = ? AND channel_id = ? AND bucket = ? AND message_id = ?",
            )
            .await?;

        Ok(Self {
            session,
            insert_msg,
            select_history,
            select_since,
            insert_bucket,
            select_buckets,
            update_content,
            soft_delete,
            upsert_read_state,
            select_read_state,
            select_read_states_for_user,
        })
    }

    /// Write a message. Returns the persisted Message with generated Snowflake ID.
    #[allow(clippy::too_many_arguments)]
    pub async fn write_message(
        &self,
        hub_id: i64,
        channel_id: i64,
        author_id: &str,
        author_type: &str,
        content: &str,
        mentions: Vec<String>,
        mention_groups: Vec<String>,
        mention_everyone: bool,
        attachments: Vec<String>,
        thread_root_id: Option<i64>,
        client_id: Option<String>,
    ) -> Result<Message> {
        let message_id = snowflake::next_id();
        let bucket = snowflake::current_bucket();

        let mentions_set: HashSet<String> = mentions.iter().cloned().collect();
        let mention_groups_set: HashSet<String> = mention_groups.iter().cloned().collect();

        // Write message (token-aware: driver routes to partition owner)
        self.session
            .execute_unpaged(
                &self.insert_msg,
                (
                    hub_id,
                    channel_id,
                    bucket,
                    message_id,
                    author_id,
                    author_type,
                    content,
                    thread_root_id,
                    &mentions_set,
                    &mention_groups_set,
                    mention_everyone,
                    &attachments,
                    &client_id,
                ),
            )
            .await?;

        // Track non-empty bucket (avoids tombstone scans on empty buckets)
        self.session
            .execute_unpaged(&self.insert_bucket, (hub_id, channel_id, bucket))
            .await?;

        Ok(Message {
            hub_id,
            channel_id,
            message_id,
            author_id: author_id.to_string(),
            author_type: author_type.to_string(),
            content: content.to_string(),
            thread_root_id,
            mentions,
            mention_groups,
            mention_everyone,
            attachments,
            edited_at: None,
            deleted_at: None,
            client_id,
            bucket,
        })
    }

    /// Read message history for a channel. Cursor-based: pass `before_id` for pagination.
    /// Automatically iterates through buckets (newest first).
    pub async fn read_history(
        &self,
        hub_id: i64,
        channel_id: i64,
        limit: i32,
        before_id: Option<i64>,
    ) -> Result<Vec<Message>> {
        // Get active buckets for this channel (newest first)
        let bucket_rows = self
            .session
            .execute_unpaged(&self.select_buckets, (hub_id, channel_id, 10i32))
            .await?;

        let buckets: Vec<i32> = bucket_rows
            .into_rows_result()?
            .rows::<(i32,)>()?
            .filter_map(|r| r.ok().map(|r| r.0))
            .collect();

        if buckets.is_empty() {
            return Ok(vec![]);
        }

        let mut messages = Vec::new();
        let remaining = limit;

        for bucket in buckets {
            if messages.len() >= remaining as usize {
                break;
            }

            // Fetch extra when cursor filtering (before_id skips rows post-query)
            let base_need = remaining - messages.len() as i32;
            let need = if before_id.is_some() { (base_need * 3).min(300) } else { base_need.min(100) };

            let rows = self
                .session
                .execute_unpaged(&self.select_history, (hub_id, channel_id, bucket, need))
                .await?;

            for row in rows.into_rows_result()?.rows::<(
                i64,    // hub_id
                i64,    // channel_id
                i32,    // bucket
                i64,    // message_id
                String, // author_id
                String, // author_type
                String, // content
                Option<i64>,            // thread_root_id
                Option<HashSet<String>>, // mentions
                Option<HashSet<String>>, // mention_groups
                Option<bool>,           // mention_everyone
                Option<Vec<String>>,    // attachments
                Option<scylla::value::CqlTimestamp>, // edited_at
                Option<scylla::value::CqlTimestamp>, // deleted_at
                Option<String>,         // client_id
            )>()? {
                let row = row?;

                // Apply cursor filter
                if let Some(before) = before_id {
                    if row.3 >= before {
                        continue;
                    }
                }

                // Skip soft-deleted
                if row.13.is_some() {
                    continue;
                }

                messages.push(Message {
                    hub_id: row.0,
                    channel_id: row.1,
                    bucket: row.2,
                    message_id: row.3,
                    author_id: row.4,
                    author_type: row.5,
                    content: row.6,
                    thread_root_id: row.7,
                    mentions: row.8.map(|s| s.into_iter().collect()).unwrap_or_default(),
                    mention_groups: row.9.map(|s| s.into_iter().collect()).unwrap_or_default(),
                    mention_everyone: row.10.unwrap_or(false),
                    attachments: row.11.unwrap_or_default(),
                    edited_at: None,
                    deleted_at: None,
                    client_id: row.14,
                });
            }
        }

        messages.truncate(remaining as usize);
        Ok(messages)
    }

    /// Read messages AFTER a given message_id (ASC order, oldest first).
    /// Used by /sync for catching up after offline.
    /// Returns (messages, limited) where limited=true means > SYNC_LIMIT messages exist.
    pub async fn read_since(
        &self,
        hub_id: i64,
        channel_id: i64,
        after_id: i64,
    ) -> Result<(Vec<Message>, bool)> {
        let bucket_rows = self
            .session
            .execute_unpaged(&self.select_buckets, (hub_id, channel_id, 30i32))
            .await?;

        let mut buckets: Vec<i32> = bucket_rows
            .into_rows_result()?
            .rows::<(i32,)>()?
            .filter_map(|r| r.ok().map(|r| r.0))
            .collect();

        // Sort ASC for chronological iteration
        buckets.sort();

        let mut messages = Vec::new();
        // Fetch SYNC_LIMIT+1 to detect overflow
        let fetch_limit = (SYNC_LIMIT + 1) as i32;

        for bucket in buckets {
            if messages.len() > SYNC_LIMIT {
                break;
            }

            let rows = self
                .session
                .execute_unpaged(
                    &self.select_since,
                    (hub_id, channel_id, bucket, after_id, fetch_limit),
                )
                .await?;

            for row in rows.into_rows_result()?.rows::<(
                i64, i64, i32, i64, String, String, String,
                Option<i64>, Option<HashSet<String>>, Option<HashSet<String>>,
                Option<bool>, Option<Vec<String>>,
                Option<scylla::value::CqlTimestamp>, Option<scylla::value::CqlTimestamp>,
                Option<String>,
            )>()? {
                let row = row?;
                if row.13.is_some() {
                    continue; // skip soft-deleted
                }
                messages.push(Message {
                    hub_id: row.0,
                    channel_id: row.1,
                    bucket: row.2,
                    message_id: row.3,
                    author_id: row.4,
                    author_type: row.5,
                    content: row.6,
                    thread_root_id: row.7,
                    mentions: row.8.map(|s| s.into_iter().collect()).unwrap_or_default(),
                    mention_groups: row.9.map(|s| s.into_iter().collect()).unwrap_or_default(),
                    mention_everyone: row.10.unwrap_or(false),
                    attachments: row.11.unwrap_or_default(),
                    edited_at: None,
                    deleted_at: None,
                    client_id: row.14,
                });
            }
        }

        let limited = messages.len() > SYNC_LIMIT;
        if limited {
            // Too many events: return only the latest SYNC_LIMITED_RETURN
            let start = messages.len().saturating_sub(SYNC_LIMITED_RETURN);
            messages = messages[start..].to_vec();
        }

        Ok((messages, limited))
    }

    /// Edit message content. Only the author should call this (checked in API layer).
    pub async fn edit_message(
        &self,
        hub_id: i64,
        channel_id: i64,
        bucket: i32,
        message_id: i64,
        new_content: &str,
        mentions: Vec<String>,
        mention_groups: Vec<String>,
        mention_everyone: bool,
    ) -> Result<()> {
        let mentions_set: HashSet<String> = mentions.into_iter().collect();
        let mention_groups_set: HashSet<String> = mention_groups.into_iter().collect();

        self.session
            .execute_unpaged(
                &self.update_content,
                (
                    new_content,
                    &mentions_set,
                    &mention_groups_set,
                    mention_everyone,
                    hub_id,
                    channel_id,
                    bucket,
                    message_id,
                ),
            )
            .await?;
        Ok(())
    }

    /// Soft delete: set deleted_at, clear content.
    pub async fn delete_message(
        &self,
        hub_id: i64,
        channel_id: i64,
        bucket: i32,
        message_id: i64,
    ) -> Result<()> {
        self.session
            .execute_unpaged(
                &self.soft_delete,
                (hub_id, channel_id, bucket, message_id),
            )
            .await?;
        Ok(())
    }

    // ── Read State (ScyllaDB source of truth) ──────

    /// Mark channel as read up to message_id. Write-through to ScyllaDB.
    pub async fn mark_read(
        &self,
        user_id: &str,
        hub_id: i64,
        channel_id: i64,
        last_read_message_id: i64,
    ) -> Result<()> {
        self.session
            .execute_unpaged(
                &self.upsert_read_state,
                (user_id, hub_id, channel_id, last_read_message_id),
            )
            .await?;
        Ok(())
    }

    /// Get read state for a single channel. Returns (last_read_message_id, mention_count).
    pub async fn get_read_state(
        &self,
        user_id: &str,
        hub_id: i64,
        channel_id: i64,
    ) -> Result<Option<(i64, i32)>> {
        let rows = self
            .session
            .execute_unpaged(
                &self.select_read_state,
                (user_id, hub_id, channel_id),
            )
            .await?;

        let result = rows
            .into_rows_result()?
            .rows::<(i64, i32)>()?
            .next()
            .transpose()?;

        Ok(result)
    }

    /// Get all read states for a user (for initial sync / cache warm-up).
    /// Returns Vec<(hub_id, channel_id, last_read_message_id, mention_count)>.
    pub async fn get_all_read_states(
        &self,
        user_id: &str,
    ) -> Result<Vec<(i64, i64, i64, i32)>> {
        let rows = self
            .session
            .execute_unpaged(
                &self.select_read_states_for_user,
                (user_id,),
            )
            .await?;

        let result: Vec<(i64, i64, i64, i32)> = rows
            .into_rows_result()?
            .rows::<(i64, i64, i64, i32)>()?
            .filter_map(|r| r.ok())
            .collect();

        Ok(result)
    }
}
