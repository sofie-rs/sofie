-- The principal whose action queued the message, so its job writes as them; null for none.
ALTER TABLE sofie_messages ADD COLUMN actor uuid;
