-- The hourly sweep reads only the last day's forgotten principals, so it finds
-- them without a scan of a table that only grows.
CREATE INDEX CONCURRENTLY IF NOT EXISTS forgotten_principals_at ON forgotten_principals (forgotten_at);
