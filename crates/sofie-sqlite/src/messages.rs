use sofie_db::{MessageSql, Migrations};

/// Named as Postgres's are, so an app moving between them finds each one applied once.
pub(crate) const MIGRATIONS: Migrations = Migrations::new(
    "sofie",
    sofie_db::embed!(
        "migrations",
        [
            "0001_messages.sql",
            "0002_idempotency.sql",
            "0003_history.sql",
            "0004_flags.sql",
            "0008_settings.sql",
            "0009_local_schedules.sql",
            "0010_notify.sql",
            "0012_mail_suppressions.sql",
            "0013_messages_waiting.sql",
            "0014_mirror_forgotten.sql",
            "0015_mirror_forgotten_at.sql",
            "0016_history_action.sql",
            "0017_messages_actor.sql",
            "0018_previous_slug.sql",
            "0019_schedule_cadence.sql",
            "0020_inbox_read.sql",
            "0021_notify_digest_items_sent.sql",
            "0022_forgotten_principals.sql"
        ]
    ),
);

macro_rules! now {
    () => {
        "CAST(unixepoch('subsec') * 1000000 AS INTEGER)"
    };
}

// One writer at a time claims, so no row is skipped as locked.
pub(crate) const MESSAGES: MessageSql = MessageSql {
    insert: "INSERT INTO sofie_messages (id, kind, name, payload, available_at, traceparent, actor)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    fan_out: "INSERT INTO sofie_messages
    (id, kind, name, payload, available_at, traceparent, actor, event_id)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
ON CONFLICT (event_id, name) DO NOTHING",
    // Materialized, the join gets an automatic index where two `json_each` would loop per pair.
    // `WHERE true` tells the upsert's `ON CONFLICT` from a join's `ON`.
    fan_out_events: "WITH event AS MATERIALIZED (
    SELECT value ->> '$.id' AS id, value -> '$.payload' AS payload,
        value ->> '$.traceparent' AS traceparent
    FROM json_each(?1)
), job AS MATERIALIZED (
    SELECT value ->> '$.id' AS id, value ->> '$.name' AS name, value ->> '$.event' AS event
    FROM json_each(?2)
)
INSERT INTO sofie_messages
    (id, kind, name, payload, available_at, traceparent, actor, event_id)
SELECT job.id, 'job', job.name, event.payload, ?3, event.traceparent, ?4, event.id
FROM event JOIN job ON job.event = event.id
WHERE true
ON CONFLICT (event_id, name) DO NOTHING",
    claim: concat!(
        "UPDATE sofie_messages
SET claimed_until = ",
        now!(),
        " + ?1 * 1000, attempts = attempts + 1
WHERE id IN (
    SELECT id FROM sofie_messages
    WHERE dead_at IS NULL
        AND available_at <= ?2
        AND (claimed_until IS NULL OR claimed_until <= ",
        now!(),
        ")
        AND ((kind = 'event' AND name IN (SELECT value FROM json_each(?4, '$.event')))
            OR (kind = 'job' AND name IN (SELECT value FROM json_each(?4, '$.job')))
            OR available_at <= ?5)
    ORDER BY available_at
    LIMIT ?3
)
RETURNING id, kind, name, payload, attempts, traceparent, available_at, actor"
    ),
    renew: concat!(
        "UPDATE sofie_messages SET claimed_until = ",
        now!(),
        " + ?3 * 1000
WHERE id = ?1 AND attempts = ?2 AND dead_at IS NULL"
    ),
    complete: "DELETE FROM sofie_messages WHERE id = ?1 AND attempts = ?2",
    retry: "UPDATE sofie_messages SET claimed_until = NULL, last_error = ?3, available_at = ?4
WHERE id = ?1 AND attempts = ?2",
    bury: concat!(
        "UPDATE sofie_messages SET claimed_until = NULL, last_error = ?3, dead_at = ",
        now!(),
        ", event_id = NULL
WHERE id = ?1 AND attempts = ?2"
    ),
    to_job: "UPDATE sofie_messages SET kind = 'job', name = ?3, event_id = id
WHERE id = ?1 AND attempts = ?2",
    // A schedule stored before cadences were kept adopts this one and keeps its time.
    add_schedule: "INSERT INTO sofie_schedules (name, next_at, cadence) VALUES (?1, ?2, ?3)
ON CONFLICT (name) DO UPDATE SET cadence = excluded.cadence,
    next_at = CASE WHEN sofie_schedules.cadence IS NULL THEN sofie_schedules.next_at
        ELSE excluded.next_at END
WHERE sofie_schedules.cadence IS NOT excluded.cadence",
    // Three statements on one connection, which no other writer interleaves:
    // the message is queued only when the schedule moved and no run of it is still waiting, and
    // its id tells so.
    fire_schedule: "UPDATE sofie_schedules SET next_at = ?8 WHERE name = ?3 AND next_at <= ?5;
INSERT INTO sofie_messages (id, kind, name, payload, available_at, traceparent, actor)
SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7 WHERE changes() > 0
    AND NOT EXISTS (SELECT 1 FROM sofie_messages WHERE name = ?3 AND kind = 'job' AND dead_at IS NULL);
SELECT next_at, EXISTS (SELECT 1 FROM sofie_messages WHERE id = ?1)
FROM sofie_schedules WHERE name = ?3",
    notify: None,
};

#[cfg(test)]
mod tests {
    use super::MESSAGES;

    #[test]
    fn fanning_out_events_indexes_one_side_instead_of_scanning_both_per_pair() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection
            .execute_batch(include_str!("../migrations/0001_messages.sql"))
            .unwrap();
        connection
            .execute_batch(include_str!("../migrations/0017_messages_actor.sql"))
            .unwrap();
        let explain = format!("EXPLAIN QUERY PLAN {}", MESSAGES.fan_out_events);
        let mut statement = connection.prepare(&explain).unwrap();
        let plan: Vec<String> = statement
            .query_map(("[]", "[]", 0, rusqlite::types::Null), |row| row.get(3))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        assert!(
            plan.iter().any(|step| step.contains("AUTOMATIC")),
            "{plan:?}"
        );
    }
}
