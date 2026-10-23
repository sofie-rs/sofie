CREATE TABLE sofie_settings (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    name text NOT NULL UNIQUE,
    value text NOT NULL,
    changed_at timestamptz NOT NULL
);
