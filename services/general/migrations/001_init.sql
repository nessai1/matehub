-- MateHub schema: Hub -> Channel -> Member

CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- ── Hubs ─────────────────────────────────────────
CREATE TABLE IF NOT EXISTS hubs (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name        TEXT NOT NULL,
    slug        TEXT NOT NULL UNIQUE,
    plan        TEXT NOT NULL DEFAULT 'free',  -- free, pro, enterprise
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ── Users ────────────────────────────────────────
CREATE TABLE IF NOT EXISTS users (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    username    TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    avatar_url  TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ── Hub members (many-to-many) ───────────────────
CREATE TABLE IF NOT EXISTS hub_members (
    hub_id      UUID NOT NULL REFERENCES hubs(id) ON DELETE CASCADE,
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role        TEXT NOT NULL DEFAULT 'member',  -- admin, member, guest
    joined_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (hub_id, user_id)
);

-- ── Channels ─────────────────────────────────────
CREATE TABLE IF NOT EXISTS channels (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    hub_id      UUID NOT NULL REFERENCES hubs(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    type        TEXT NOT NULL DEFAULT 'text',  -- text, voice, stage
    position    INT NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_channels_hub ON channels(hub_id);
