-- One row per runtime setting staff changed: its value as JSON (sofie-flags
-- reads it); a setting without one holds its declared default.
CREATE TABLE settings (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    name text NOT NULL UNIQUE,
    value text NOT NULL,
    changed_at timestamptz NOT NULL
);
