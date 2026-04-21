CREATE TABLE hub_members (
    hub_id      UUID NOT NULL REFERENCES hubs(id) ON DELETE CASCADE,
    account_id  UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    role        TEXT NOT NULL,
    added_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (hub_id, account_id),
    CONSTRAINT hub_members_role_valid CHECK (role IN ('owner', 'admin', 'member'))
);

CREATE INDEX hub_members_by_account ON hub_members (account_id);
