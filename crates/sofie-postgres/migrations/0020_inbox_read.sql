-- Read entries are pruned by age every hour, so the prune finds them without a scan.
CREATE INDEX CONCURRENTLY IF NOT EXISTS inbox_read ON inbox (read_at) WHERE read_at IS NOT NULL;
