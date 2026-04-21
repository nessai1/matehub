CREATE TABLE hubs (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    slug              TEXT NOT NULL UNIQUE,
    name              TEXT NOT NULL,
    owner_account_id  UUID NOT NULL REFERENCES accounts(id) ON DELETE RESTRICT,
    status            TEXT NOT NULL DEFAULT 'provisioning',
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT hubs_slug_format CHECK (slug ~ '^[a-z0-9][a-z0-9-]{1,62}[a-z0-9]$'),
    CONSTRAINT hubs_status_valid CHECK (status IN ('provisioning', 'ready', 'failed', 'deleting'))
);

CREATE INDEX hubs_owner ON hubs (owner_account_id);
