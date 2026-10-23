-- Principals whose product events were erased. A batch the writer still held
-- when one was forgotten skips its events, and the hourly prune deletes any a
-- batch racing the erasure wrote, then the entries older than a day.
CREATE TABLE forgotten_principals (
    principal uuid PRIMARY KEY,
    forgotten_at timestamptz NOT NULL DEFAULT now()
);
