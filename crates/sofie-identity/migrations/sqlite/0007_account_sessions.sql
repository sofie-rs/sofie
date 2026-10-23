CREATE TABLE sofie_identity_account_session (
    id uuid PRIMARY KEY NOT NULL,
    version bigint NOT NULL,
    account uuid NOT NULL REFERENCES sofie_identity_account (id),
    session uuid NOT NULL,
    area text NOT NULL,
    device text,
    last_seen_at timestamptz NOT NULL,
    ended_at timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE UNIQUE INDEX sofie_identity_account_session_session
    ON sofie_identity_account_session (session);
CREATE INDEX sofie_identity_account_session_account
    ON sofie_identity_account_session (account, ended_at);
