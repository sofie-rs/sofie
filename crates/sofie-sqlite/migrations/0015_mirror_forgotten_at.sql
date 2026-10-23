ALTER TABLE sofie_mirror_forgotten ADD COLUMN forgotten_at timestamptz NOT NULL DEFAULT 0;
UPDATE sofie_mirror_forgotten SET forgotten_at = CAST(unixepoch('subsec') * 1000000 AS INTEGER);
CREATE INDEX sofie_mirror_forgotten_at ON sofie_mirror_forgotten (forgotten_at);
