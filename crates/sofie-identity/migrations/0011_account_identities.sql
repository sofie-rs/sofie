-- An account's identities at OAuth and OpenID Connect providers, each signing it in.
CREATE TABLE account_identity (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    account uuid NOT NULL REFERENCES account (id),
    provider text NOT NULL,
    subject text NOT NULL,
    email text,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    deleted_at timestamptz
);
CREATE UNIQUE INDEX account_identity_subject ON account_identity (provider, subject)
    WHERE deleted_at IS NULL;
CREATE INDEX account_identity_account ON account_identity (account);
