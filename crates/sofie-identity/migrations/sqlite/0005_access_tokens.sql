CREATE TABLE sofie_identity_access_token (
    id uuid PRIMARY KEY NOT NULL,
    version bigint NOT NULL,
    account uuid NOT NULL REFERENCES sofie_identity_account (id),
    area text NOT NULL,
    digest text NOT NULL,
    scopes text NOT NULL,
    expires_at timestamptz NOT NULL,
    last_used_at timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    deleted_at timestamptz
);
CREATE UNIQUE INDEX sofie_identity_access_token_digest ON sofie_identity_access_token (digest);
CREATE INDEX sofie_identity_access_token_account ON sofie_identity_access_token (account);
