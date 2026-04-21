CREATE TABLE mail_outbox (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    template      TEXT NOT NULL,
    to_email      TEXT NOT NULL,
    payload       JSONB NOT NULL,
    status        TEXT NOT NULL DEFAULT 'pending',
    attempts      INT NOT NULL DEFAULT 0,
    last_error    TEXT,
    message_id    TEXT,
    scheduled_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    sent_at       TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT mail_outbox_status_valid
        CHECK (status IN ('pending', 'sending', 'sent', 'failed'))
);

CREATE INDEX mail_outbox_pending
    ON mail_outbox (scheduled_at)
    WHERE status = 'pending';
