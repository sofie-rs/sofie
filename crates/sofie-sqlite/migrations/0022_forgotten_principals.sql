-- Principals an erasure forgot, kept so work queued before it writes nothing for them.
CREATE TABLE sofie_forgotten_principals (
    principal uuid PRIMARY KEY,
    forgotten_at timestamptz NOT NULL DEFAULT (CAST(unixepoch('subsec') * 1000000 AS INTEGER))
);
