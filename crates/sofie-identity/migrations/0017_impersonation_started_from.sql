-- The impersonator's own session it started from, so stopping signs back in only while that
-- session lives; null for impersonations started before, which stop by signing out.
ALTER TABLE impersonation ADD COLUMN started_from uuid;
