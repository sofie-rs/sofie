-- Bearer tokens, stored only as the digest of their text.
CREATE TABLE access_token (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    account uuid NOT NULL REFERENCES account (id),
    area text NOT NULL,
    digest text NOT NULL,
    scopes text NOT NULL,
    expires_at timestamptz NOT NULL,
    last_used_at timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    deleted_at timestamptz
);
CREATE UNIQUE INDEX access_token_digest ON access_token (digest);
CREATE INDEX access_token_account ON access_token (account);
