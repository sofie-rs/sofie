-- A closed account's events are deleted by principal, and the hourly prune
-- joins the forgotten principals on it, so neither scans the table.
CREATE INDEX CONCURRENTLY IF NOT EXISTS product_events_principal ON product_events (principal);
