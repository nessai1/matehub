-- ── General invite links ───────────────────────────
--
-- Third invite mechanism alongside `temp_users` (TTL guests) and
-- `invitations` (admin-pre-allocated permanent users). One link is shared
-- with many people; each visitor self-registers (login + password +
-- display_name + email) and lands as a real users row.
--
-- The link is alive while:
--   revoked_at IS NULL
--   AND expires_at > now()
--   AND (max_uses IS NULL OR uses_count < max_uses)
--
-- Slot claim is atomic: redeem path runs
--   UPDATE invite_links SET uses_count = uses_count + 1
--    WHERE token = $1 AND <alive predicate> RETURNING ...
-- Zero rows back = link no longer alive (race-loser on last slot, expired,
-- or revoked). The increment lives inside the same tx as the user INSERT,
-- so a username UNIQUE collision rolls back the slot too — collisions
-- never burn a slot.

CREATE TABLE IF NOT EXISTS invite_links (
    id          BIGINT PRIMARY KEY,
    hub_id      BIGINT NOT NULL REFERENCES hubs(id) ON DELETE CASCADE,
    token       TEXT NOT NULL UNIQUE,
    expires_at  TIMESTAMPTZ NOT NULL,
    -- NULL = unlimited uses (until expires_at).
    max_uses    INTEGER,
    uses_count  INTEGER NOT NULL DEFAULT 0,
    -- Optional: target group for redeemed users. NULL = everyone-only.
    -- ON DELETE SET NULL: removing the group later doesn't break already-
    -- issued links — the affected accounts are still in the everyone group.
    group_id    BIGINT REFERENCES groups(id) ON DELETE SET NULL,
    created_by  BIGINT NOT NULL REFERENCES users(id),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at  TIMESTAMPTZ,
    CONSTRAINT max_uses_positive CHECK (max_uses IS NULL OR max_uses > 0),
    CONSTRAINT uses_within_max   CHECK (max_uses IS NULL OR uses_count <= max_uses)
);

CREATE INDEX IF NOT EXISTS idx_invite_links_hub
    ON invite_links(hub_id) WHERE revoked_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_invite_links_token
    ON invite_links(token);

ALTER TABLE invite_links ENABLE ROW LEVEL SECURITY;
CREATE POLICY invite_links_hub_isolation ON invite_links
    USING (hub_id = current_setting('app.current_hub_id', true)::bigint);
