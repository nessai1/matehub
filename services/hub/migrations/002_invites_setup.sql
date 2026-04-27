-- Add hub avatar + permanent invitations for the setup wizard / "Add
-- Teammates" flow. Temp users (already in 001) cover guest links; this
-- migration covers the matching permanent-account flow.

-- ── Hub icon ────────────────────────────────────────
ALTER TABLE hubs ADD COLUMN IF NOT EXISTS avatar_url TEXT;

-- ── Permanent invitations ──────────────────────────
-- One row per invite link. `username` is allocated up front (admin picks
-- the login when creating the invite). `email` is informational only —
-- boxed has no SMTP, so the inviter copies the link manually.
-- Single-use: `used_at` flips from NULL to now() on acceptance, after
-- which the link returns 410 Gone.
CREATE TABLE IF NOT EXISTS invitations (
    id          BIGINT PRIMARY KEY,
    hub_id      BIGINT NOT NULL REFERENCES hubs(id) ON DELETE CASCADE,
    token       TEXT NOT NULL UNIQUE,
    username    TEXT NOT NULL,
    email       TEXT,
    group_id    BIGINT REFERENCES groups(id) ON DELETE SET NULL,
    created_by  BIGINT NOT NULL REFERENCES users(id),
    used_at     TIMESTAMPTZ,
    used_by     BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_invitations_hub
    ON invitations(hub_id) WHERE used_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_invitations_token
    ON invitations(token);

-- The `username` column is the future users.username, and that table
-- has a global UNIQUE — but the invite isn't bound to that user yet.
-- Allow multiple pending invites for the same username (one of them
-- will win on accept; the others will fail on users.username unique).
-- That's fine: race-loser sees a clean error.

ALTER TABLE invitations ENABLE ROW LEVEL SECURITY;
CREATE POLICY invitations_hub_isolation ON invitations
    USING (hub_id = current_setting('app.current_hub_id', true)::bigint);
