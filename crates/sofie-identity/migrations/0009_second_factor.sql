-- A code from an authenticator app asked after the password, and the pending sign-in it guards.
CREATE TABLE second_factor (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    account uuid NOT NULL REFERENCES account (id),
    secret text NOT NULL,
    confirmed_at timestamptz,
    last_step bigint NOT NULL,
    recovery text NOT NULL,
    pending_recovery text,
    failures integer NOT NULL,
    failing_since timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE UNIQUE INDEX second_factor_account ON second_factor (account);

ALTER TABLE account_link DROP CONSTRAINT account_link_purpose_check;
ALTER TABLE account_link ADD CONSTRAINT account_link_purpose_check
    CHECK (purpose IN ('PASSWORD_RESET', 'EMAIL_VERIFICATION', 'SIGN_IN', 'SECOND_FACTOR'));
