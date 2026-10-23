CREATE TABLE sofie_mirror_forgotten (
    mirror text NOT NULL,
    id uuid NOT NULL,
    version bigint NOT NULL,
    PRIMARY KEY (mirror, id)
);
