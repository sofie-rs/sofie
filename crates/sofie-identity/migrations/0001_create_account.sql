CREATE TABLE account (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    email text NOT NULL,
    password_hash text NOT NULL
);
CREATE UNIQUE INDEX account_email ON account (email);

CREATE TABLE membership (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    account uuid NOT NULL REFERENCES account (id),
    area text NOT NULL
);
CREATE UNIQUE INDEX membership_account_area ON membership (account, area);
