-- The shield's blocks, shared by every instance when the app turns sharing
-- on: one row per blocked caller until its block ends. Rows past `until`
-- are ignored and replaced by the next block of the same caller.
CREATE TABLE shield (
    key text PRIMARY KEY,
    rule text NOT NULL,
    grade text NOT NULL,
    until timestamptz NOT NULL
);
CREATE INDEX shield_until ON shield (until);
