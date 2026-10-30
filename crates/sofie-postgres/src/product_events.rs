use sofie_db::ProductSql;

/// Periods are cut in UTC; a month's offset counts calendar months, a day's
/// or a week's whole periods between the two starts.
pub(crate) const PRODUCT_EVENTS: ProductSql = ProductSql {
    insert: "INSERT INTO sofie.product_events (at, name, principal)
SELECT event.* FROM unnest($1::timestamptz[], $2::text[], $3::uuid[]) AS event (at, name, principal)
WHERE event.principal IS NULL OR NOT EXISTS (
    SELECT 1 FROM sofie.forgotten_principals forgotten WHERE forgotten.principal = event.principal
)",
    cohorts: "WITH cohort AS (
    SELECT principal, date_trunc($1, min(at) AT TIME ZONE 'UTC') AS start
    FROM sofie.product_events
    WHERE name = $2 AND principal IS NOT NULL AND at >= $4
    GROUP BY principal
), returned AS (
    SELECT DISTINCT cohort.principal, cohort.start,
        date_trunc($1, event.at AT TIME ZONE 'UTC') AS period
    FROM cohort
    JOIN sofie.product_events event ON event.principal = cohort.principal
    WHERE event.name = $3 AND event.at AT TIME ZONE 'UTC' >= cohort.start
)
SELECT start, -1::int8, count(*) FROM cohort GROUP BY start
UNION ALL
SELECT start,
    CASE $1
        WHEN 'month' THEN ((extract(year FROM period) - extract(year FROM start)) * 12
            + extract(month FROM period) - extract(month FROM start))::int8
        ELSE (extract(epoch FROM period - start)
            / CASE $1 WHEN 'week' THEN 604800 ELSE 86400 END)::int8
    END,
    count(DISTINCT principal)
FROM returned
GROUP BY 1, 2
ORDER BY 1, 2",
    prune: "WITH deleted AS (
    DELETE FROM sofie.product_events WHERE ctid IN (
        SELECT ctid FROM sofie.product_events WHERE at < $1 LIMIT 10000
    ) RETURNING 1
) SELECT count(*) FROM deleted",
    forget: "DELETE FROM sofie.product_events WHERE principal = $1",
    // Later writes skip the principals kept forgotten, so only the last day's are swept.
    sweep: "DELETE FROM sofie.product_events event USING sofie.forgotten_principals forgotten
WHERE event.principal = forgotten.principal AND forgotten.forgotten_at > now() - interval '1 day'",
};
