CREATE TABLE "order" (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    email text NOT NULL,
    status text NOT NULL,
    total bigint NOT NULL,
    total_currency char(3) NOT NULL
);
