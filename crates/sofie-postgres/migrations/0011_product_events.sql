-- Product events kept for retention and cohort reports, written in batches
-- with sofie's `product-reports`; empty in an app without it. Appended in
-- time order, so a BRIN index serves pruning by age.
CREATE TABLE product_events (
    at timestamptz NOT NULL,
    name text NOT NULL,
    principal uuid
);
CREATE INDEX product_events_name ON product_events (name, principal, at);
CREATE INDEX product_events_at ON product_events USING brin (at);
