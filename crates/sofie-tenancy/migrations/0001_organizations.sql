-- Organizations own records as an account does; their members hold a role each, and an
-- invitation by email becomes a member once accepted.
CREATE TABLE organization (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    name text NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);

CREATE TABLE organization_member (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    organization uuid NOT NULL REFERENCES organization (id),
    account uuid NOT NULL,
    role text NOT NULL CHECK (role IN ('OWNER', 'ADMIN', 'MEMBER')),
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    deleted_at timestamptz
);
CREATE UNIQUE INDEX organization_member_account ON organization_member (organization, account)
    WHERE deleted_at IS NULL;
CREATE INDEX organization_member_by_account ON organization_member (account);

CREATE TABLE invitation (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    organization uuid NOT NULL REFERENCES organization (id),
    email text NOT NULL,
    role text NOT NULL CHECK (role IN ('OWNER', 'ADMIN', 'MEMBER')),
    invited_by uuid NOT NULL,
    expires_at timestamptz NOT NULL,
    accepted_at timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE INDEX invitation_organization ON invitation (organization, accepted_at);
