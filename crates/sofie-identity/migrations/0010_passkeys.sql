-- Public keys of an account's passkeys (WebAuthn credentials).
CREATE TABLE passkey (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    account uuid NOT NULL REFERENCES account (id),
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
CREATE UNIQUE INDEX passkey_credential ON passkey (credential);
CREATE INDEX passkey_account ON passkey (account);
