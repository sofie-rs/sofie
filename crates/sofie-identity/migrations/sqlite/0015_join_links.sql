-- SQLite cannot change a check, so the table is copied into one with the new purpose.
CREATE TABLE sofie_identity_account_link_joining (
    id uuid PRIMARY KEY NOT NULL,
    version bigint NOT NULL,
    account uuid NOT NULL REFERENCES sofie_identity_account (id),
    purpose text NOT NULL
        CHECK (purpose IN ('PASSWORD_RESET', 'EMAIL_VERIFICATION', 'SIGN_IN', 'SECOND_FACTOR', 'JOIN')),
    expires_at timestamptz NOT NULL,
    used_at timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
INSERT INTO sofie_identity_account_link_joining
    (id, version, account, purpose, expires_at, used_at, created_at, updated_at)
    SELECT id, version, account, purpose, expires_at, used_at, created_at, updated_at
    FROM sofie_identity_account_link;
DROP TABLE sofie_identity_account_link;
ALTER TABLE sofie_identity_account_link_joining RENAME TO sofie_identity_account_link;
CREATE INDEX sofie_identity_account_link_account
    ON sofie_identity_account_link (account, purpose, created_at);
