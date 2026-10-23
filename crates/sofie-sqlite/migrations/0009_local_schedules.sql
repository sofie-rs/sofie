CREATE TABLE sofie_local_schedules (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    key text NOT NULL UNIQUE,
    job text NOT NULL,
    payload text NOT NULL,
    at timestamp NOT NULL,
    at_zone text NOT NULL,
    repeat text NOT NULL,
    next_at timestamptz,
    owner uuid
);
CREATE INDEX sofie_local_schedules_due ON sofie_local_schedules (next_at) WHERE next_at IS NOT NULL;
