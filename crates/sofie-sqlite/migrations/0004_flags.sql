CREATE TABLE sofie_flags (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    name text NOT NULL UNIQUE,
    rule text NOT NULL,
    changed_at timestamptz NOT NULL
);
