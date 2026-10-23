ALTER TABLE sofie_identity_account_session ADD COLUMN expires_at timestamptz;

CREATE TABLE sofie_identity_impersonation (
    id uuid PRIMARY KEY NOT NULL,
    version bigint NOT NULL,
    account uuid NOT NULL REFERENCES sofie_identity_account (id),
    impersonator uuid NOT NULL REFERENCES sofie_identity_account (id),
    impersonator_email text NOT NULL,
    area text NOT NULL,
    reason text NOT NULL,
    session uuid NOT NULL,
    ends_at timestamptz NOT NULL,
    ended_at timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE INDEX sofie_identity_impersonation_session ON sofie_identity_impersonation (session);
CREATE INDEX sofie_identity_impersonation_created ON sofie_identity_impersonation (created_at);
