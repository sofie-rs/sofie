-- Quotas count from here, as period_start moves on every renewal.
ALTER TABLE subscription ADD COLUMN started_at timestamptz;
