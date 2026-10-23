-- Swept past the longest a late change event is retried for.
ALTER TABLE mirror_forgotten ADD COLUMN forgotten_at timestamptz NOT NULL DEFAULT now();
CREATE INDEX mirror_forgotten_at ON mirror_forgotten (forgotten_at);
