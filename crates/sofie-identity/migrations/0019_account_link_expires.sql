-- Expired links are pruned hourly.
CREATE INDEX CONCURRENTLY IF NOT EXISTS account_link_expires ON account_link (expires_at);
