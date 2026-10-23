CREATE TABLE sofie_identity_account_identity (
    id uuid PRIMARY KEY NOT NULL,
    version bigint NOT NULL,
    account uuid NOT NULL REFERENCES sofie_identity_account (id),
    provider text NOT NULL,
    subject text NOT NULL,
    email text,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    deleted_at timestamptz
);
CREATE UNIQUE INDEX sofie_identity_account_identity_subject
    ON sofie_identity_account_identity (provider, subject) WHERE deleted_at IS NULL;
CREATE INDEX sofie_identity_account_identity_account
    ON sofie_identity_account_identity (account);
