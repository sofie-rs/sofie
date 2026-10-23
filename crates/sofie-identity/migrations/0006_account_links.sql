-- When the owner confirmed the address through a mailed link; null until then.
ALTER TABLE account ADD COLUMN verified_at timestamptz;

-- Links mailed to an account's address, each used once.
CREATE TABLE account_link (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    account uuid NOT NULL REFERENCES account (id),
    purpose text NOT NULL CHECK (purpose IN ('PASSWORD_RESET', 'EMAIL_VERIFICATION')),
    expires_at timestamptz NOT NULL,
    used_at timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE INDEX account_link_account ON account_link (account, purpose, created_at);
