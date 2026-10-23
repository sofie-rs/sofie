-- One row per feature flag anyone switched: its rule as JSON (sofie-flags reads
-- it) and when it last changed, which tells how long it has been settled.
CREATE TABLE flags (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    name text NOT NULL UNIQUE,
    rule text NOT NULL,
    changed_at timestamptz NOT NULL
);
