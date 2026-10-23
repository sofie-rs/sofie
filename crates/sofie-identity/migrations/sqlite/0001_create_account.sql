CREATE TABLE sofie_identity_account (
    id uuid PRIMARY KEY NOT NULL,
    version bigint NOT NULL,
    email text NOT NULL,
    password_hash text NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE UNIQUE INDEX sofie_identity_account_email ON sofie_identity_account (email);

CREATE TABLE sofie_identity_membership (
    id uuid PRIMARY KEY NOT NULL,
    version bigint NOT NULL,
    account uuid NOT NULL REFERENCES sofie_identity_account (id),
    area text NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE UNIQUE INDEX sofie_identity_membership_account_area
    ON sofie_identity_membership (account, area);
