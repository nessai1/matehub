-- Hub-level permission bits stored per group (bits 7-13).
-- Per-channel bits (0-6) remain in channel_permissions table.
ALTER TABLE groups ADD COLUMN IF NOT EXISTS hub_permissions INT NOT NULL DEFAULT 0;

-- Track who created the hub (can't be removed from admin).
ALTER TABLE hubs ADD COLUMN IF NOT EXISTS creator_id UUID REFERENCES users(id);

-- Set admin group hub_permissions to ALL (16383) for existing hubs.
UPDATE groups SET hub_permissions = 16383 WHERE name = 'admin';

-- Set creator_id to first admin member for existing hubs.
UPDATE hubs h SET creator_id = (
    SELECT mg.user_id FROM member_groups mg
    JOIN groups g ON g.id = mg.group_id AND g.hub_id = h.id
    WHERE g.name = 'admin'
    ORDER BY mg.user_id
    LIMIT 1
) WHERE creator_id IS NULL;
