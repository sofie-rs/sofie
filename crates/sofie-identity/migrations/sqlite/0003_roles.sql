CREATE TABLE sofie_identity_role (
    id uuid PRIMARY KEY NOT NULL,
    version bigint NOT NULL,
    name text NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE UNIQUE INDEX sofie_identity_role_name ON sofie_identity_role (name);

CREATE TABLE sofie_identity_role_grant (
    id uuid PRIMARY KEY NOT NULL,
    version bigint NOT NULL,
    role uuid NOT NULL REFERENCES sofie_identity_role (id),
    permission text NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    deleted_at timestamptz
);
CREATE UNIQUE INDEX sofie_identity_role_grant_permission
    ON sofie_identity_role_grant (role, permission) WHERE deleted_at IS NULL;

ALTER TABLE sofie_identity_membership ADD COLUMN role uuid REFERENCES sofie_identity_role (id);
