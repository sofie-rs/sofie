-- Jobs at a wall-clock time in a zone (sofie-db's `LocalJobs`): the local time
-- and zone are kept, and each run is placed by the zone's rules when it is due.
CREATE TABLE local_schedules (
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
CREATE INDEX local_schedules_due ON local_schedules (next_at) WHERE next_at IS NOT NULL;
