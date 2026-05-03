use anyhow::Result;
use scylla::client::session::Session;
use scylla::client::session_builder::SessionBuilder;
use std::sync::Arc;

pub type ScyllaPool = Arc<Session>;

const KEYSPACE_CQL: &str = r#"
CREATE KEYSPACE IF NOT EXISTS matehub_chat
WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1}
AND durable_writes = true
"#;

const MESSAGES_CQL: &str = r#"
CREATE TABLE IF NOT EXISTS matehub_chat.messages (
    hub_id         bigint,
    channel_id     bigint,
    bucket         int,
    message_id     bigint,
    author_id      text,
    author_type    text,
    content        text,
    thread_root_id bigint,
    mentions       frozen<set<text>>,
    mention_groups frozen<set<text>>,
    mention_everyone boolean,
    attachments    frozen<list<text>>,
    edited_at      timestamp,
    deleted_at     timestamp,
    client_id      text,
    PRIMARY KEY ((hub_id, channel_id, bucket), message_id)
) WITH CLUSTERING ORDER BY (message_id DESC)
   AND gc_grace_seconds = 172800
   AND compaction = {'class': 'TimeWindowCompactionStrategy', 'compaction_window_size': 7, 'compaction_window_unit': 'DAYS'}
"#;

const REACTIONS_CQL: &str = r#"
CREATE TABLE IF NOT EXISTS matehub_chat.reactions (
    hub_id     bigint,
    channel_id bigint,
    message_id bigint,
    emoji      text,
    user_id    text,
    created_at timestamp,
    PRIMARY KEY ((hub_id, channel_id, message_id), emoji, user_id)
)
"#;

const READ_STATE_CQL: &str = r#"
CREATE TABLE IF NOT EXISTS matehub_chat.read_state (
    user_id              text,
    hub_id               bigint,
    channel_id           bigint,
    last_read_message_id bigint,
    mention_count        int,
    updated_at           timestamp,
    PRIMARY KEY ((user_id), hub_id, channel_id)
)
"#;

const CHANNEL_BUCKETS_CQL: &str = r#"
CREATE TABLE IF NOT EXISTS matehub_chat.channel_buckets (
    hub_id     bigint,
    channel_id bigint,
    bucket     int,
    PRIMARY KEY ((hub_id, channel_id), bucket)
) WITH CLUSTERING ORDER BY (bucket DESC)
"#;

// Attachment index: look up by id → S3 url + metadata.
// Attachments are stored inline in messages.attachments (list<text>) for the
// chat render path; this table is a sidecar index for the streaming proxy so
// `/v1/attachments/{id}/stream` doesn't have to scan messages. Hub_id is
// duplicated here so the stream handler can authorize without joining.
const ATTACHMENTS_CQL: &str = r#"
CREATE TABLE IF NOT EXISTS matehub_chat.attachments (
    attachment_id text PRIMARY KEY,
    hub_id        bigint,
    channel_id    bigint,
    message_id    bigint,
    bucket        int,
    url           text,
    content_type  text,
    size          bigint
)
"#;

pub async fn connect(scylla_url: &str) -> Result<ScyllaPool> {
    let session = SessionBuilder::new().known_node(scylla_url).build().await?;

    tracing::info!(%scylla_url, "ScyllaDB connected");
    Ok(Arc::new(session))
}

pub async fn migrate(session: &ScyllaPool) -> Result<()> {
    session.query_unpaged(KEYSPACE_CQL, &[]).await?;
    tracing::info!("ScyllaDB keyspace created");

    session.query_unpaged(MESSAGES_CQL, &[]).await?;
    session.query_unpaged(REACTIONS_CQL, &[]).await?;
    session.query_unpaged(READ_STATE_CQL, &[]).await?;
    session.query_unpaged(CHANNEL_BUCKETS_CQL, &[]).await?;
    session.query_unpaged(ATTACHMENTS_CQL, &[]).await?;
    tracing::info!("ScyllaDB tables created");

    session.use_keyspace("matehub_chat", false).await?;
    tracing::info!("ScyllaDB using keyspace matehub_chat");

    Ok(())
}
