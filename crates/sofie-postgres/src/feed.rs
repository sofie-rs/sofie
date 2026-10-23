//! The event feed's statements, beside `migrations/0007_feed.sql`, which creates its tables.

use sofie_db::FeedSql;

pub(crate) const FEED: FeedSql = FeedSql {
    // The lock is held to the commit, so a position never commits before a
    // smaller one: a reader past it has seen every earlier entry.
    append: "WITH ordered AS (SELECT pg_advisory_xact_lock(4611904032514410071))
INSERT INTO sofie.feed (id, name, payload, traceparent)
SELECT $1::uuid, $2::text, $3::jsonb, $4::text FROM ordered
ON CONFLICT (id) DO NOTHING",
    read: "SELECT position, id, name, payload, traceparent FROM sofie.feed
WHERE position > $1 ORDER BY position LIMIT $2",
    prune: "DELETE FROM sofie.feed WHERE created_at < $1",
    receive: "INSERT INTO sofie.messages (id, kind, name, payload, available_at, traceparent)
VALUES ($1, 'event', $2, $3, $4, $5)
ON CONFLICT (id) DO NOTHING",
    cursor: "SELECT position FROM sofie.feed_cursors WHERE producer = $1",
    advance: "INSERT INTO sofie.feed_cursors (producer, position) VALUES ($1, $3)
ON CONFLICT (producer) DO UPDATE SET position = excluded.position
WHERE sofie.feed_cursors.position = $2",
};
