-- Read entries are pruned by age every hour, so the prune finds them without a scan.
CREATE INDEX sofie_inbox_read ON sofie_inbox (read_at) WHERE read_at IS NOT NULL;
