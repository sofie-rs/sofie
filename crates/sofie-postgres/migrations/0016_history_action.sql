-- The route or job that made the change; null for one made outside both.
ALTER TABLE history ADD COLUMN action text;
