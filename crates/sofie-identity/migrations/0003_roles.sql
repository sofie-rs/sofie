-- Staff roles and the permissions each grants; a membership of a
-- `Permitted` area names the role its member holds there.
CREATE TABLE role (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    name text NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE UNIQUE INDEX role_name ON role (name);

CREATE TABLE role_grant (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    role uuid NOT NULL REFERENCES role (id),
    permission text NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    deleted_at timestamptz
);
CREATE UNIQUE INDEX role_grant_held ON role_grant (role, permission) WHERE deleted_at IS NULL;

ALTER TABLE membership ADD COLUMN role uuid REFERENCES role (id);
