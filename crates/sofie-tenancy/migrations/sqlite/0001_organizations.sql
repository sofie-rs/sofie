CREATE TABLE sofie_tenancy_organization (
    id uuid PRIMARY KEY NOT NULL,
    version bigint NOT NULL,
    name text NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);

CREATE TABLE sofie_tenancy_organization_member (
    id uuid PRIMARY KEY NOT NULL,
    version bigint NOT NULL,
    organization uuid NOT NULL REFERENCES sofie_tenancy_organization (id),
    account uuid NOT NULL,
    role text NOT NULL CHECK (role IN ('OWNER', 'ADMIN', 'MEMBER')),
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    deleted_at timestamptz
);
CREATE UNIQUE INDEX sofie_tenancy_organization_member_account
    ON sofie_tenancy_organization_member (organization, account) WHERE deleted_at IS NULL;
CREATE INDEX sofie_tenancy_organization_member_by_account
    ON sofie_tenancy_organization_member (account);

CREATE TABLE sofie_tenancy_invitation (
    id uuid PRIMARY KEY NOT NULL,
    version bigint NOT NULL,
    organization uuid NOT NULL REFERENCES sofie_tenancy_organization (id),
    email text NOT NULL,
    role text NOT NULL CHECK (role IN ('OWNER', 'ADMIN', 'MEMBER')),
    invited_by uuid NOT NULL,
    expires_at timestamptz NOT NULL,
    accepted_at timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE INDEX sofie_tenancy_invitation_organization
    ON sofie_tenancy_invitation (organization, accepted_at);
