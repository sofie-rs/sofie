CREATE TABLE sofie_idempotency (
    key text PRIMARY KEY,
    fingerprint text NOT NULL,
    created_at timestamptz NOT NULL,
    answered_at timestamptz,
    reply jsonb
);
CREATE INDEX sofie_idempotency_created ON sofie_idempotency (created_at);
