-- Signed-in sessions, so their owner can list them and end one from elsewhere.
CREATE TABLE account_session (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    account uuid NOT NULL REFERENCES account (id),
    session uuid NOT NULL,
    area text NOT NULL,
    device text,
    last_seen_at timestamptz NOT NULL,
    ended_at timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE UNIQUE INDEX account_session_session ON account_session (session);
CREATE INDEX account_session_account ON account_session (account, ended_at);
