CREATE TABLE sofie_mail_suppressions (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    address text NOT NULL,
    scope text NOT NULL,
    reason text NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    UNIQUE (address, scope)
);
