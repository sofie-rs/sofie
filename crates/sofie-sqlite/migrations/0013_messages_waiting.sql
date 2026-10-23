-- Whether a schedule's run still waits, read without scanning every live message.
CREATE INDEX sofie_messages_waiting ON sofie_messages (name) WHERE kind = 'job' AND dead_at IS NULL;
