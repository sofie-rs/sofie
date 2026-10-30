use std::future::poll_fn;
use std::pin::pin;
use std::task::Poll;

use sofie_core::{Error, Result};
use sofie_db::{BoxFuture, MessageSql, Migrations, Wakeups};
use tokio_postgres::{AsyncMessage, Client, Connection, Socket};

use crate::Postgres;
use crate::error::classified;
use crate::tls;

type Listening = Connection<Socket, tls::Stream>;

pub(crate) const MIGRATIONS: Migrations = Migrations::new(
    "sofie",
    sofie_db::embed!(
        "migrations",
        [
            "0001_messages.sql",
            "0002_idempotency.sql",
            "0003_history.sql",
            "0004_flags.sql",
            "0005_shield.sql",
            "0006_module_roles.sql",
            "0007_feed.sql",
            "0008_settings.sql",
            "0009_local_schedules.sql",
            "0010_notify.sql",
            "0011_product_events.sql",
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
            "0022_forgotten_principals.sql",
            "0023_product_events_principal.sql",
            "0024_forgotten_principals_at.sql"
        ]
    ),
);

pub(crate) const LISTEN: &str = "LISTEN sofie_messages";

// `attempts` doubles as the lease's fencing token: every later write names
// the value this claim set. `ARRAY(…)` runs the locking subquery once, where
// `IN (…)` may run it again and claim past the limit.
pub(crate) const MESSAGES: MessageSql = MessageSql {
    insert: "INSERT INTO sofie.messages (id, kind, name, payload, available_at, traceparent, actor)
VALUES ($1, $2, $3, $4, $5, $6, $7)",
    fan_out: "INSERT INTO sofie.messages
    (id, kind, name, payload, available_at, traceparent, actor, event_id)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
ON CONFLICT (event_id, name) DO NOTHING",
    // A recordset reads a JSON null payload as SQL NULL.
    fan_out_events: "INSERT INTO sofie.messages
    (id, kind, name, payload, available_at, traceparent, actor, event_id)
SELECT job.id, 'job', job.name, COALESCE(event.payload, 'null'), $3::timestamptz, event.traceparent,
    $4::uuid, event.id
FROM jsonb_to_recordset($1::jsonb) AS event (id uuid, payload jsonb, traceparent text)
JOIN jsonb_to_recordset($2::jsonb) AS job (id uuid, name text, event uuid) ON job.event = event.id
ON CONFLICT (event_id, name) DO NOTHING",
    claim: "UPDATE sofie.messages
SET claimed_until = now() + $1::bigint * interval '1 millisecond', attempts = attempts + 1
WHERE id = ANY(ARRAY(
    SELECT id FROM sofie.messages
    WHERE dead_at IS NULL
        AND available_at <= $2
        AND (claimed_until IS NULL OR claimed_until <= now())
        AND (($4::jsonb -> kind) ? name OR available_at <= $5)
    ORDER BY available_at
    LIMIT $3
    FOR UPDATE SKIP LOCKED
))
RETURNING id, kind, name, payload, attempts, traceparent, available_at, actor",
    renew: "UPDATE sofie.messages
SET claimed_until = now() + $3::bigint * interval '1 millisecond'
WHERE id = $1 AND attempts = $2 AND dead_at IS NULL",
    complete: "DELETE FROM sofie.messages WHERE id = $1 AND attempts = $2",
    retry: "UPDATE sofie.messages SET claimed_until = NULL, last_error = $3, available_at = $4
WHERE id = $1 AND attempts = $2",
    bury: "UPDATE sofie.messages
SET claimed_until = NULL, last_error = $3, dead_at = now(), event_id = NULL
WHERE id = $1 AND attempts = $2",
    to_job: "UPDATE sofie.messages SET kind = 'job', name = $3, event_id = id
WHERE id = $1 AND attempts = $2",
    // A schedule stored before cadences were kept, or under the same cadence's older key,
    // adopts this one and keeps its time.
    add_schedule: "INSERT INTO sofie.schedules (name, next_at, cadence) VALUES ($1, $2, $3)
ON CONFLICT (name) DO UPDATE SET cadence = excluded.cadence,
    next_at = CASE WHEN schedules.cadence IS NULL OR schedules.cadence = $4::text
        THEN schedules.next_at ELSE excluded.next_at END
WHERE schedules.cadence IS DISTINCT FROM excluded.cadence",
    // The outer select reads the table as it was before the update, so a
    // schedule that was not due answers its stored time. A run still waiting
    // or retrying holds back the next, so a failing schedule does not pile up.
    fire_schedule: "WITH due AS (
    UPDATE sofie.schedules SET next_at = $8 WHERE name = $3 AND next_at <= $5
    RETURNING name, next_at
), fired AS (
    INSERT INTO sofie.messages (id, kind, name, payload, available_at, traceparent, actor)
    SELECT $1::uuid, $2::text, name, $4::jsonb, $5::timestamptz, $6::text, $7::uuid FROM due
    WHERE NOT EXISTS (
        SELECT 1 FROM sofie.messages WHERE name = $3 AND kind = 'job' AND dead_at IS NULL
    )
    RETURNING id
)
SELECT next_at, EXISTS (SELECT 1 FROM fired) FROM due
UNION ALL
SELECT next_at, false FROM sofie.schedules
WHERE name = $3 AND NOT EXISTS (SELECT 1 FROM due)",
    notify: Some("NOTIFY sofie_messages"),
};

impl Postgres {
    pub(crate) fn subscribe(
        &self,
        listen: &'static str,
    ) -> BoxFuture<'_, Result<Box<dyn Wakeups>>> {
        Box::pin(async move { Ok(Box::new(self.listening(listen).await?) as Box<dyn Wakeups>) })
    }

    #[cfg(any(feature = "cache", feature = "live"))]
    pub(crate) fn notifications(
        &self,
        listen: &'static str,
    ) -> BoxFuture<'_, Result<Box<dyn sofie_db::Notifications>>> {
        Box::pin(async move {
            Ok(Box::new(self.listening(listen).await?) as Box<dyn sofie_db::Notifications>)
        })
    }

    #[cfg_attr(
        not(feature = "tls"),
        expect(
            clippy::clone_on_copy,
            reason = "`NoTls` is `Copy`, rustls's connector is not"
        )
    )]
    async fn listening(&self, listen: &'static str) -> Result<Notifications> {
        let (client, mut connection) = self
            .config
            .connect(self.tls.clone())
            .await
            .map_err(|error| classified("opening the listening connection", error))?;
        let early = listen_on(&client, &mut connection, listen).await?;
        Ok(Notifications {
            _client: client,
            connection,
            early,
        })
    }
}

struct Notifications {
    _client: Client,
    connection: Listening,
    /// Arrived while LISTEN completed, so the first `next` answers them at once.
    early: Vec<String>,
}

impl Notifications {
    fn next_payloads(&mut self) -> impl Future<Output = Result<Vec<String>>> + Send + '_ {
        let mut payloads = std::mem::take(&mut self.early);
        poll_fn(move |context| {
            match next_notification(&mut self.connection, context, &mut payloads) {
                Poll::Pending if payloads.is_empty() => Poll::Pending,
                Poll::Pending => Poll::Ready(Ok(std::mem::take(&mut payloads))),
                Poll::Ready(polled) => Poll::Ready(polled.map(|()| std::mem::take(&mut payloads))),
            }
        })
    }
}

/// The connection does the client's I/O, so it is driven until LISTEN returns.
async fn listen_on(
    client: &Client,
    connection: &mut Listening,
    statement: &str,
) -> Result<Vec<String>> {
    let mut listen = pin!(client.batch_execute(statement));
    let mut early = Vec::new();
    poll_fn(|context| {
        if let Poll::Ready(result) = listen.as_mut().poll(context) {
            return Poll::Ready(result.map_err(listen_error));
        }
        match next_notification(connection, context, &mut early) {
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            _ => Poll::Pending,
        }
    })
    .await?;
    Ok(early)
}

impl Wakeups for Notifications {
    fn next(&mut self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move { self.next_payloads().await.map(drop) })
    }
}

#[cfg(any(feature = "cache", feature = "live"))]
impl sofie_db::Notifications for Notifications {
    fn next(&mut self) -> BoxFuture<'_, Result<Vec<String>>> {
        Box::pin(self.next_payloads())
    }
}

/// Consumes every buffered notification, so a burst wakes the worker once.
fn next_notification(
    connection: &mut Listening,
    context: &mut std::task::Context<'_>,
    payloads: &mut Vec<String>,
) -> Poll<Result<()>> {
    let mut notified = false;
    loop {
        match connection.poll_message(context) {
            Poll::Ready(Some(Ok(AsyncMessage::Notification(notification)))) => {
                notified = true;
                payloads.push(notification.payload().to_owned());
            }
            Poll::Ready(Some(Ok(_))) => {}
            Poll::Ready(Some(Err(error))) => return Poll::Ready(Err(listen_error(error))),
            Poll::Ready(None) => {
                return Poll::Ready(Err(Error::internal_message(
                    "the listening connection closed",
                )));
            }
            Poll::Pending if notified => return Poll::Ready(Ok(())),
            Poll::Pending => return Poll::Pending,
        }
    }
}

fn listen_error(error: tokio_postgres::Error) -> Error {
    classified("listening for notifications", error)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use sofie_testing::TestDatabase;

    use super::*;

    #[tokio::test]
    async fn a_notification_arriving_while_listen_completes_wakes_the_first_wait() {
        let database = TestDatabase::create().await;
        let postgres = Postgres::connect(database.url()).unwrap();
        let mut wakeups = postgres
            .subscribe("LISTEN sofie_messages; NOTIFY sofie_messages")
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(1), wakeups.next())
            .await
            .expect("the early notification woke it")
            .unwrap();
    }
}
