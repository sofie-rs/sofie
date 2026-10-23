-- Sent digest items are pruned by age every hour, so the prune finds them without a scan.
CREATE INDEX CONCURRENTLY IF NOT EXISTS notify_digest_items_sent ON notify_digest_items (sent_at)
WHERE sent_at IS NOT NULL;
