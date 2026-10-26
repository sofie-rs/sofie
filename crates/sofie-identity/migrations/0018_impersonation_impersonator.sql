-- Ending all of an account's sessions ends the impersonations it started.
CREATE INDEX CONCURRENTLY IF NOT EXISTS impersonation_impersonator ON impersonation (impersonator, ended_at);
