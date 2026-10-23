-- The first answer to each idempotent request and webhook delivery, replayed
-- for a day; sofie-db sweeps expired keys and takes them over.
CREATE TABLE idempotency (
    key text PRIMARY KEY,
    fingerprint text NOT NULL,
    created_at timestamptz NOT NULL,
    answered_at timestamptz,
    reply jsonb
);
CREATE INDEX idempotency_created ON idempotency (created_at);
