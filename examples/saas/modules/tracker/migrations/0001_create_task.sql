CREATE TABLE "task" (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    organization uuid NOT NULL,
    title text NOT NULL CHECK (char_length(title) BETWEEN 1 AND 120),
    status text NOT NULL CHECK (status IN ('OPEN', 'DONE'))
);
CREATE INDEX task_organization ON "task" (organization);
