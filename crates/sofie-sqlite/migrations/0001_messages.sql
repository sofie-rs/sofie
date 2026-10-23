CREATE TABLE sofie_messages (
    id uuid PRIMARY KEY,
    kind text NOT NULL,
    name text NOT NULL,
    payload jsonb NOT NULL,
    available_at timestamptz NOT NULL,
    traceparent text,
    event_id uuid,
    attempts integer NOT NULL DEFAULT 0,
    claimed_until timestamptz,
    last_error text,
    dead_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT (CAST(unixepoch('subsec') * 1000000 AS INTEGER)),
    UNIQUE (event_id, name)
);
-- Dead messages stay until someone looks at them; claiming never reads them.
CREATE INDEX sofie_messages_due ON sofie_messages (available_at) WHERE dead_at IS NULL;

CREATE TABLE sofie_schedules (
    name text PRIMARY KEY,
    next_at timestamptz NOT NULL
);
