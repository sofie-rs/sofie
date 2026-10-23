-- Whether a schedule's run still waits, read without scanning every live message.
CREATE INDEX CONCURRENTLY IF NOT EXISTS messages_waiting ON messages (name)
WHERE kind = 'job' AND dead_at IS NULL;
