# MateHub -- Permission System

## Overview

Permissions are stored as **bitfields** (i32). Two levels:

- **Per-channel** (bits 0-6): stored in `channel_permissions.allow_bits / deny_bits` per group per channel
- **Hub-level** (bits 7-13): stored in `groups.hub_permissions` per group

A user's effective permissions = union of all their groups' bits. Per-channel: `(all allows OR'd) AND NOT (all denies OR'd)`. Hub-level: `all groups' hub_permissions OR'd`.

Admin group bypasses all checks (hardcoded, not bit-dependent).

## Permission Bits

### Per-channel (bits 0-6)

| Name | Bit | Value | Description |
|------|-----|-------|-------------|
| READ | 0 | 1 | See channel, read messages |
| WRITE | 1 | 2 | Send messages |
| CONNECT | 2 | 4 | Join voice channel |
| SPEAK | 3 | 8 | Unmute mic in voice |
| VIDEO | 4 | 16 | Enable camera in voice |
| MANAGE_CHANNEL | 5 | 32 | Edit channel settings, kick from voice |
| ADMIN_CHANNEL | 6 | 64 | Delete others' messages, manage channel permissions |

### Hub-level (bits 7-13)

| Name | Bit | Value | Description |
|------|-----|-------|-------------|
| CREATE_TEXT_CHANNELS | 7 | 128 | Create text channels |
| CREATE_VOICE_CHANNELS | 8 | 256 | Create voice channels |
| EDIT_OTHER_CHANNELS | 9 | 512 | Edit/delete channels created by others |
| MANAGE_MEMBERS | 10 | 1024 | Kick members (only those with lower group position) |
| INVITE_PERMANENT | 11 | 2048 | Send email invites for permanent accounts |
| CREATE_TEMP_LINKS | 12 | 4096 | Create temporary magic-link invites |
| MANAGE_ROLES | 13 | 8192 | Create/edit/delete groups, assign members to groups |

### Presets

| Name | Value | Used by |
|------|-------|---------|
| ALL | 16383 | admin group |
| MEMBER_CHANNEL | 31 | everyone group (per-channel defaults) |

14 of 31 available bits used. 17 slots free for future expansion. Upgrade path: ALTER COLUMN to i64 for 63 slots.

## Group Hierarchy

Groups have a `position` field (lower number = higher privilege). Rules:

1. **Role management**: a user with MANAGE_ROLES can only create/edit/delete groups with position > their highest group's position.
2. **Bit delegation**: a user can only grant permission bits that they themselves possess. Admin can grant any.
3. **Kick**: MANAGE_MEMBERS allows kicking members whose highest group position > caller's highest position.
4. **Role assignment**: a user can only add/remove others to/from groups with position > their own.

## Protected Entities

### Groups
- **admin**: cannot be deleted. Only name and color can be edited. hub_permissions and position are immutable.
- **everyone**: same restrictions as admin. `is_default = true` -- auto-assigned to new members.
- Other groups: full CRUD by anyone with MANAGE_ROLES + sufficient position.

### Hub Creator
- Stored in `hubs.creator_id`.
- Cannot be removed from the admin group.
- Cannot be kicked from the hub.
- Only the creator can delete the hub (hardcoded, no bit).

### Hub Settings
- Editing hub name/icon/slug: admin-only (hardcoded, no bit).

## Backend Enforcement

**Every mutating endpoint MUST check permissions.** Read endpoints are protected by RLS (row-level security on hub_id).

### Resolution

```
api/auth_check.rs:
  resolve_user_perms(pool, hub_id, user_id) -> UserPerms {
    hub_bits: i32,        // union of all group hub_permissions
    top_position: i32,    // lowest position number (= highest privilege)
    is_admin: bool,       // member of "admin" group
    is_creator: bool,     // hubs.creator_id == user_id
  }
```

`UserPerms.has(bit)` returns true if admin OR bit is set.
`UserPerms.can_manage_position(target_pos)` returns true if admin OR own position < target.
`UserPerms.grantable_bits()` returns ALL for admin, own bits otherwise.

### Endpoint Enforcement Matrix

| Endpoint | Required Permission | Extra Checks |
|----------|-------------------|--------------|
| `POST /channels` (text) | CREATE_TEXT_CHANNELS | -- |
| `POST /channels` (voice/stage) | CREATE_VOICE_CHANNELS | -- |
| `PATCH /channels/{id}` | owner or EDIT_OTHER_CHANNELS | -- |
| `DELETE /channels/{id}` | EDIT_OTHER_CHANNELS | can't delete position=0 text channel |
| `POST /groups` | MANAGE_ROLES | bits <= grantable, position below caller |
| `PATCH /groups/{id}` | MANAGE_ROLES | position check; admin/everyone: name/color only |
| `DELETE /groups/{id}` | MANAGE_ROLES | not admin/everyone, position below caller |
| `POST /groups/{id}/members/{uid}` | MANAGE_ROLES | group position below caller |
| `DELETE /groups/{id}/members/{uid}` | MANAGE_ROLES | position check; can't remove creator from admin |
| `DELETE /members/{uid}` (kick) | MANAGE_MEMBERS | target position below caller; not creator |
| `POST /temp-users` | CREATE_TEMP_LINKS | -- |
| `POST /invite` | INVITE_PERMANENT | -- |
| `PATCH /hubs/{id}` | admin only (hardcoded) | -- |
| `DELETE /hubs/{id}` | creator only (hardcoded) | -- |

### Per-channel Permission Check (for chat/video services)

```
GET /hubs/{hub_id}/channels/{channel_id}/effective/{user_id}

Returns: { bits, read, write, connect, speak, video, manage, admin }
```

Chat service calls this (or caches) to verify READ/WRITE before accepting messages. Video service checks CONNECT/SPEAK/VIDEO before allowing join.

## Database Schema

```sql
-- Groups with hub-level permissions
CREATE TABLE groups (
    id              UUID PRIMARY KEY,
    hub_id          UUID NOT NULL REFERENCES hubs(id),
    name            TEXT NOT NULL,
    color           TEXT,
    position        INT NOT NULL DEFAULT 0,
    is_default      BOOLEAN NOT NULL DEFAULT false,
    hub_permissions INT NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Per-channel permission overrides per group
CREATE TABLE channel_permissions (
    channel_id  UUID NOT NULL REFERENCES channels(id),
    group_id    UUID NOT NULL REFERENCES groups(id),
    allow_bits  INT NOT NULL DEFAULT 0,
    deny_bits   INT NOT NULL DEFAULT 0,
    PRIMARY KEY (channel_id, group_id)
);

-- Hub creator tracking
ALTER TABLE hubs ADD COLUMN creator_id UUID REFERENCES users(id);
```

## Adding New Permissions

1. Add constant to `models/permission.rs` (next power of 2: bit 14 = 16384).
2. Add enforcement in the relevant API handler.
3. Update this document.
4. No migration needed -- bits stored in existing INT columns.
