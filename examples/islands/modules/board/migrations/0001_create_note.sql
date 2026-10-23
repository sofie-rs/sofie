CREATE TABLE note (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    title text NOT NULL,
    body text NOT NULL
);
