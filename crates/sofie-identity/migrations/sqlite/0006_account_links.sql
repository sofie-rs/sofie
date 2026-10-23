ALTER TABLE sofie_identity_account ADD COLUMN verified_at timestamptz;

-- With every purpose later migrations add on Postgres, as SQLite cannot change a check.
CREATE TABLE sofie_identity_account_link (
    id uuid PRIMARY KEY NOT NULL,
    version bigint NOT NULL,
    account uuid NOT NULL REFERENCES sofie_identity_account (id),
    purpose text NOT NULL
        CHECK (purpose IN ('PASSWORD_RESET', 'EMAIL_VERIFICATION', 'SIGN_IN', 'SECOND_FACTOR')),
    expires_at timestamptz NOT NULL,
    used_at timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE INDEX sofie_identity_account_link_account
    ON sofie_identity_account_link (account, purpose, created_at);
