-- The principal whose action raised the event and the area they acted in, so the consuming app's
-- listener runs as them; null for none.
ALTER TABLE feed ADD COLUMN actor uuid, ADD COLUMN actor_area text;
