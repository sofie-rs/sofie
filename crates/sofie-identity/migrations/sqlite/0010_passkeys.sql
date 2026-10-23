-- A removed passkey's credential may be added again, as on Postgres since 0013.
CREATE TABLE sofie_identity_passkey (
    id uuid PRIMARY KEY NOT NULL,
    version bigint NOT NULL,
    account uuid NOT NULL REFERENCES sofie_identity_account (id),
    credential text NOT NULL,
    public_key text NOT NULL,
    algorithm integer NOT NULL,
    sign_count bigint NOT NULL,
    challenged_at bigint NOT NULL,
    last_used_at timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    deleted_at timestamptz
);
CREATE UNIQUE INDEX sofie_identity_passkey_credential
    ON sofie_identity_passkey (credential) WHERE deleted_at IS NULL;
CREATE INDEX sofie_identity_passkey_account ON sofie_identity_passkey (account);
