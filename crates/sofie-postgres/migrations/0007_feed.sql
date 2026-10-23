-- Events this app publishes to other apps, in the order their appends
-- committed, read by position; and how far this app has read each other
-- app's feed. Both stay empty in an app that was never split.
CREATE TABLE feed (
    position bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id uuid NOT NULL UNIQUE,
    name text NOT NULL,
    payload jsonb NOT NULL,
    traceparent text,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX feed_created ON feed (created_at);

CREATE TABLE feed_cursors (
    producer text PRIMARY KEY,
    position bigint NOT NULL
);
