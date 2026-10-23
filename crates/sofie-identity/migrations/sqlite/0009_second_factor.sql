CREATE TABLE sofie_identity_second_factor (
    id uuid PRIMARY KEY NOT NULL,
    version bigint NOT NULL,
    account uuid NOT NULL REFERENCES sofie_identity_account (id),
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
CREATE UNIQUE INDEX sofie_identity_second_factor_account
    ON sofie_identity_second_factor (account);
