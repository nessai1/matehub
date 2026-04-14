-- Presence: only last_seen stored in Postgres.
-- Online status lives in Redis (TTL-based, not DB).

ALTER TABLE hub_members ADD COLUMN IF NOT EXISTS last_seen_at TIMESTAMPTZ;
