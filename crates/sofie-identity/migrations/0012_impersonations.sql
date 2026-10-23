-- Each time a permitted principal signs in as a member, kept for audit; the session it
-- opens ends by itself.
ALTER TABLE account_session ADD COLUMN expires_at timestamptz;

CREATE TABLE impersonation (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    account uuid NOT NULL REFERENCES account (id),
    impersonator uuid NOT NULL REFERENCES account (id),
    impersonator_email text NOT NULL,
    area text NOT NULL,
    reason text NOT NULL,
    session uuid NOT NULL,
    ends_at timestamptz NOT NULL,
    ended_at timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE INDEX impersonation_session ON impersonation (session);
CREATE INDEX impersonation_created ON impersonation (created_at);
