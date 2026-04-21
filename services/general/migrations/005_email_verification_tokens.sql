CREATE TABLE email_verification_tokens (
    token_hash   TEXT PRIMARY KEY,
    account_id   UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    expires_at   TIMESTAMPTZ NOT NULL,
    consumed_at  TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX email_verification_tokens_account
    ON email_verification_tokens (account_id)
    WHERE consumed_at IS NULL;
