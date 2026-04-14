-- Groups, channel permissions, temporary users, RLS isolation

-- ── Groups (replace text-based role) ────────────────
CREATE TABLE IF NOT EXISTS groups (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    hub_id      UUID NOT NULL REFERENCES hubs(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    color       TEXT,               -- hex color for UI (#FF5733)
    position    INT NOT NULL DEFAULT 0,
    is_default  BOOLEAN NOT NULL DEFAULT false,  -- auto-assigned to new members
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_groups_hub ON groups(hub_id);
-- One default group per hub
CREATE UNIQUE INDEX IF NOT EXISTS idx_groups_hub_default ON groups(hub_id) WHERE is_default = true;

-- ── Member <-> Group (many-to-many) ─────────────────
CREATE TABLE IF NOT EXISTS member_groups (
    hub_id      UUID NOT NULL REFERENCES hubs(id) ON DELETE CASCADE,
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    group_id    UUID NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    PRIMARY KEY (hub_id, user_id, group_id)
);

-- ── Channel permissions per group ───────────────────
-- Bitfield permissions:
--   1  = READ          (see channel, read messages)
--   2  = WRITE         (send messages)
--   4  = CONNECT       (join voice)
--   8  = SPEAK         (unmute mic in voice)
--  16  = VIDEO         (enable camera in voice)
--  32  = MANAGE        (edit channel settings, kick from voice)
--  64  = ADMIN         (delete messages, manage permissions)
CREATE TABLE IF NOT EXISTS channel_permissions (
    channel_id  UUID NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    group_id    UUID NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    allow_bits  INT NOT NULL DEFAULT 0,
    deny_bits   INT NOT NULL DEFAULT 0,
    PRIMARY KEY (channel_id, group_id)
);

-- ── Temporary users ─────────────────────────────────
CREATE TABLE IF NOT EXISTS temp_users (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    hub_id          UUID NOT NULL REFERENCES hubs(id) ON DELETE CASCADE,
    token           TEXT NOT NULL UNIQUE,   -- URL-safe random token (the magic link secret)
    nickname        TEXT NOT NULL,
    group_id        UUID NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    created_by      UUID NOT NULL REFERENCES users(id),  -- permanent user who created the link
    expires_at      TIMESTAMPTZ NOT NULL,
    revoked_at      TIMESTAMPTZ,           -- set when manually revoked
    active_session  TEXT,                   -- current WS session ID (null = nobody connected)
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_temp_users_hub ON temp_users(hub_id);
CREATE INDEX IF NOT EXISTS idx_temp_users_token ON temp_users(token);

-- ── Row-Level Security ──────────────────────────────
-- Every request sets: SET LOCAL app.current_hub_id = '<uuid>'
-- RLS ensures no cross-hub data leakage even if WHERE is forgotten.

ALTER TABLE groups ENABLE ROW LEVEL SECURITY;
CREATE POLICY groups_hub_isolation ON groups
    USING (hub_id = current_setting('app.current_hub_id', true)::uuid);

ALTER TABLE channels ENABLE ROW LEVEL SECURITY;
CREATE POLICY channels_hub_isolation ON channels
    USING (hub_id = current_setting('app.current_hub_id', true)::uuid);

ALTER TABLE hub_members ENABLE ROW LEVEL SECURITY;
CREATE POLICY hub_members_hub_isolation ON hub_members
    USING (hub_id = current_setting('app.current_hub_id', true)::uuid);

ALTER TABLE member_groups ENABLE ROW LEVEL SECURITY;
CREATE POLICY member_groups_hub_isolation ON member_groups
    USING (hub_id = current_setting('app.current_hub_id', true)::uuid);

ALTER TABLE channel_permissions ENABLE ROW LEVEL SECURITY;
CREATE POLICY channel_perms_hub_isolation ON channel_permissions
    USING (
        EXISTS (
            SELECT 1 FROM channels c
            WHERE c.id = channel_permissions.channel_id
              AND c.hub_id = current_setting('app.current_hub_id', true)::uuid
        )
    );

ALTER TABLE temp_users ENABLE ROW LEVEL SECURITY;
CREATE POLICY temp_users_hub_isolation ON temp_users
    USING (hub_id = current_setting('app.current_hub_id', true)::uuid);

-- Note: hubs and users tables do NOT have RLS --
-- hubs is the root entity (looked up by slug/id before hub_id is known)
-- users are global (shared across hubs, owned by common service)
