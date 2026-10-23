use std::time::Duration;

use serde_json::{Value, json};
use sofie_core::variants::{self, Variants};
use sofie_core::{Cast, ColumnReader, Error, Result, SqlValue, Timestamp, Uuid};
use sofie_telemetry::TraceParent;

use crate::heard::{Known, Listeners};
use crate::{BoxFuture, Db, Dialect, Stamp, Step, Wakeups, query_decoded, query_values};

/// Every insert binds the message columns first, as `$1` to `$7`: id, kind,
/// name, payload, `available_at`, traceparent, actor.
pub struct MessageSql {
    pub insert: &'static str,
    /// Message columns, then a key; skipped while a job of that name waits for it.
    pub fan_out: &'static str,
    /// `(events, jobs, available_at, actor)`: events `{id, payload, traceparent}`, jobs
    /// `{id, name, event}`.
    pub fan_out_events: &'static str,
    /// `(lease_milliseconds, now, limit, mine, held_until)`, counting attempts past the last;
    /// `mine` names by kind, and any other waits until `held_until`.
    pub claim: &'static str,
    /// `(id, attempts, lease_milliseconds)`: extends a claim still held, on the database's clock.
    pub renew: &'static str,
    pub complete: &'static str,
    pub retry: &'static str,
    /// Clears the key, so a dead job holds back no new one for it.
    pub bury: &'static str,
    /// `(id, attempts, name)`: turns an event its lone listener ran into that listener's job.
    pub to_job: &'static str,
    pub add_schedule: &'static str,
    /// Message columns, then the next time; returns it and whether this call queued a run (none
    /// while one waits or retries), no row when the schedule does not exist.
    pub fire_schedule: &'static str,
    /// Sent after a commit that queued messages, never within it: Postgres
    /// serializes the commits of notifying transactions on one lock.
    pub notify: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageKind {
    Event,
    Job,
}

impl Variants for MessageKind {
    const TYPE: &'static str = "message kind";
    const VARIANTS: &'static [(&'static str, Self)] = &[("event", Self::Event), ("job", Self::Job)];
}

/// Its `attempt` fences every later write, so an expired lease can neither finish nor fail it.
#[derive(Debug)]
pub struct Claimed {
    pub id: Uuid,
    pub kind: MessageKind,
    pub name: String,
    pub payload: Value,
    pub attempt: i64,
    pub traceparent: Option<TraceParent>,
    pub available_at: Timestamp,
    pub actor: Option<Uuid>,
}

impl Claimed {
    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            id: row.uuid()?,
            kind: variants::decode(row)?,
            name: row.text()?.to_owned(),
            payload: row.json()?,
            attempt: row.int()?,
            traceparent: traceparent(row)?,
            available_at: row.timestamp()?,
            actor: Option::<Uuid>::decode(row)?,
        })
    }

    fn fenced(&self, more: impl IntoIterator<Item = SqlValue>) -> Vec<SqlValue> {
        let mut params = vec![SqlValue::Uuid(self.id), SqlValue::Int(self.attempt)];
        params.extend(more);
        params
    }
}

fn traceparent(row: &mut dyn ColumnReader) -> Result<Option<TraceParent>> {
    if row.is_null()? {
        row.skip()?;
        return Ok(None);
    }
    Ok(TraceParent::parse(row.text()?))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fired {
    pub fired: bool,
    pub next_at: Timestamp,
}

impl Fired {
    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            next_at: row.timestamp()?,
            fired: row.bool()?,
        })
    }
}

struct Message<'a> {
    kind: MessageKind,
    name: &'a str,
    payload: Value,
    available_at: Timestamp,
    actor: Option<Uuid>,
}

impl Message<'_> {
    fn columns(self) -> Result<Vec<SqlValue>> {
        Ok(vec![
            SqlValue::Uuid(Uuid::now_v7()?),
            SqlValue::Text(self.kind.name().to_owned()),
            SqlValue::Text(self.name.to_owned()),
            SqlValue::Json(self.payload),
            SqlValue::Timestamp(self.available_at),
            current_traceparent(),
            self.actor.map_or(SqlValue::Null, SqlValue::Uuid),
        ])
    }
}

pub(crate) fn keyed_job(
    dialect: &dyn Dialect,
    name: &str,
    payload: Value,
    key: Uuid,
    available_at: Timestamp,
    actor: Option<Uuid>,
) -> Result<Step> {
    let mut params = Message {
        kind: MessageKind::Job,
        name,
        payload,
        available_at,
        actor,
    }
    .columns()?;
    params.push(SqlValue::Uuid(key));
    Ok(Step::statement(dialect.messages().fan_out, params, None))
}

/// Due times follow the app's clock, so a test can move them; leases follow the database's.
#[derive(Clone)]
pub struct Messages {
    db: Db,
}

impl Messages {
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    fn sql(&self) -> &'static MessageSql {
        self.db.driver().dialect().messages()
    }

    pub async fn claim(&self, lease: Duration, limit: i64) -> Result<Vec<Claimed>> {
        let now = self.db.clock().now().timestamp();
        claim_known(&self.db, lease, limit, &Known::default(), now).await
    }

    /// A conflict means the claim was lost: another worker took the message after its lease ran out.
    pub async fn renew(&self, message: &Claimed, lease: Duration) -> Result<()> {
        let params = message.fenced([milliseconds(lease)?]);
        let step = Step::statement(self.sql().renew, params, Some(1));
        self.db.transaction(vec![step]).await
    }

    /// Committed with its job's outcome, it makes the outcome happen once.
    #[must_use]
    pub fn completion(&self, message: &Claimed) -> Step {
        Step::statement(self.sql().complete, message.fenced([]), Some(1))
    }

    /// `listener` names the lone listener an event ran as; the message becomes its job.
    pub async fn retry(
        &self,
        message: &Claimed,
        listener: Option<&str>,
        error: &str,
        delay: Duration,
    ) -> Result<()> {
        let due = later(self.db.clock().now().timestamp(), delay)?;
        let due = Some(SqlValue::Timestamp(due));
        self.release(self.sql().retry, message, listener, error, due)
            .await
    }

    pub async fn bury(&self, message: &Claimed, listener: Option<&str>, error: &str) -> Result<()> {
        self.release(self.sql().bury, message, listener, error, None)
            .await
    }

    /// A lone listener's message first becomes its job, so its retries and its death name the
    /// listener that failed, not the event.
    async fn release(
        &self,
        sql: &'static str,
        message: &Claimed,
        listener: Option<&str>,
        error: &str,
        due: Option<SqlValue>,
    ) -> Result<()> {
        let params = message.fenced(std::iter::once(SqlValue::Text(error.to_owned())).chain(due));
        let mut steps = Vec::with_capacity(2);
        if let Some(listener) = listener {
            let params = message.fenced([SqlValue::Text(listener.to_owned())]);
            steps.push(Step::statement(self.sql().to_job, params, Some(1)));
        }
        steps.push(Step::statement(sql, params, Some(1)));
        self.db.transaction(steps).await
    }

    /// A schedule stored under another cadence starts over from `first_at`.
    pub async fn add_schedule(&self, name: &str, first_at: Timestamp, cadence: &str) -> Result<()> {
        let params = [
            SqlValue::Text(name.to_owned()),
            SqlValue::Timestamp(first_at),
            SqlValue::Text(cadence.to_owned()),
        ];
        let sql = self.sql().add_schedule;
        query_values(self.db.driver(), sql, &params).await.map(drop)
    }

    /// A run by hand: a running worker keeps the next due time in memory, but takes a job at once.
    pub async fn queue_schedule(&self, name: &str) -> Result<()> {
        let dialect = self.db.driver().dialect();
        let payload = Value::Object(serde_json::Map::new());
        let now = self.db.clock().now().timestamp();
        let mut steps = Vec::new();
        let queuing = Queuing {
            kind: MessageKind::Job,
            at: now,
            actor: None,
        };
        queue_steps(dialect, &queuing, [(name, payload)], None, &mut steps)?;
        // In the same transaction, as the process may end before a notification sent later.
        if let Some(notify) = dialect.messages().notify {
            steps.push(Step::Script(notify.to_owned()));
        }
        self.db.transaction(steps).await
    }

    /// Only one of any number of concurrent callers is told it fired.
    pub async fn fire_schedule(
        &self,
        name: &str,
        now: Timestamp,
        next_at: Timestamp,
    ) -> Result<Option<Fired>> {
        let payload = Value::Object(serde_json::Map::new());
        let mut params = Message {
            kind: MessageKind::Job,
            name,
            payload,
            available_at: now,
            actor: None,
        }
        .columns()?;
        params.push(SqlValue::Timestamp(next_at));
        let sql = self.sql().fire_schedule;
        let fired =
            query_decoded(self.db.driver(), sql, &params, Vec::new(), Fired::decode).await?;
        Ok(fired.into_iter().next())
    }

    pub(crate) async fn arrived(&self) {
        self.db.arrivals().arrived().await;
    }

    #[must_use]
    pub fn wakeups(&self) -> Option<BoxFuture<'_, Result<Box<dyn Wakeups>>>> {
        self.db.driver().wakeups()
    }
}

pub(crate) async fn claim_known(
    db: &Db,
    lease: Duration,
    limit: i64,
    mine: &Known,
    held_until: Timestamp,
) -> Result<Vec<Claimed>> {
    let params = [
        milliseconds(lease)?,
        SqlValue::Timestamp(db.clock().now().timestamp()),
        SqlValue::Int(limit),
        SqlValue::Json(mine.to_json()),
        SqlValue::Timestamp(held_until),
    ];
    let sql = db.driver().dialect().messages().claim;
    query_decoded(db.driver(), sql, &params, Vec::new(), Claimed::decode).await
}

pub async fn ping_queue(db: &Db) -> Result<()> {
    let messages = db.driver().dialect().qualified("sofie", "messages");
    let sql = format!("SELECT 1 FROM {messages} LIMIT 1");
    query_values(db.driver(), &sql, &[]).await.map(drop)
}

pub(crate) struct Queuing {
    kind: MessageKind,
    at: Timestamp,
    actor: Option<Uuid>,
}

impl Stamp {
    pub(crate) fn queuing(&self, kind: MessageKind, at: Timestamp) -> Queuing {
        Queuing {
            kind,
            at,
            actor: self.actor,
        }
    }
}

pub(crate) fn queue_steps<'a>(
    dialect: &dyn Dialect,
    queuing: &Queuing,
    messages: impl IntoIterator<Item = (&'a str, Value)>,
    heard: Option<&Known>,
    steps: &mut Vec<Step>,
) -> Result<bool> {
    if let Some(heard) = heard.filter(|_| queuing.kind == MessageKind::Event) {
        return fan_out_raised(dialect, queuing, messages, &heard.listeners, steps);
    }
    let mut queued = false;
    for (name, payload) in messages {
        let params = Message {
            kind: queuing.kind,
            name,
            payload,
            available_at: queuing.at,
            actor: queuing.actor,
        }
        .columns()?;
        steps.push(Step::statement(dialect.messages().insert, params, None));
        queued = true;
    }
    Ok(queued)
}

fn fan_out_raised<'a>(
    dialect: &dyn Dialect,
    queuing: &Queuing,
    events: impl IntoIterator<Item = (&'a str, Value)>,
    listeners: &Listeners,
    steps: &mut Vec<Step>,
) -> Result<bool> {
    let traceparent = sofie_telemetry::current_traceparent().map(|parent| parent.to_string());
    let mut raised = Vec::new();
    for (name, payload) in events {
        raised.push(Fanned {
            id: Uuid::now_v7()?,
            name,
            payload,
            traceparent: traceparent.clone(),
        });
    }
    let Some(step) = fan_out_events(dialect, raised, queuing.at, queuing.actor, listeners)? else {
        return Ok(false);
    };
    steps.push(step);
    Ok(true)
}

pub(crate) struct Fanned<'a> {
    pub(crate) id: Uuid,
    pub(crate) name: &'a str,
    pub(crate) payload: Value,
    pub(crate) traceparent: Option<String>,
}

pub(crate) fn fan_out_events<'a>(
    dialect: &dyn Dialect,
    events: impl IntoIterator<Item = Fanned<'a>>,
    at: Timestamp,
    actor: Option<Uuid>,
    listeners: &Listeners,
) -> Result<Option<Step>> {
    let (mut taken, mut jobs) = (Vec::new(), Vec::new());
    for event in events {
        let taking = listeners.taking(event.name, &event.payload);
        if taking.is_empty() {
            continue;
        }
        let id = event.id.to_string();
        for listener in taking.iter() {
            let job = Uuid::now_v7()?.to_string();
            jobs.push(json!({ "id": job, "name": listener, "event": id }));
        }
        taken.push(json!({ "id": id, "payload": event.payload, "traceparent": event.traceparent }));
    }
    if jobs.is_empty() {
        return Ok(None);
    }
    let params = vec![
        SqlValue::Json(Value::Array(taken)),
        SqlValue::Json(Value::Array(jobs)),
        SqlValue::Timestamp(at),
        actor.map_or(SqlValue::Null, SqlValue::Uuid),
    ];
    let sql = dialect.messages().fan_out_events;
    Ok(Some(Step::statement(sql, params, None)))
}

pub(crate) fn current_traceparent() -> SqlValue {
    sofie_telemetry::current_traceparent().map_or(SqlValue::Null, |traceparent| {
        SqlValue::Text(traceparent.to_string())
    })
}

fn milliseconds(lease: Duration) -> Result<SqlValue> {
    i64::try_from(lease.as_millis())
        .map(SqlValue::Int)
        .map_err(|error| Error::internal("a lease too long to store", error))
}

fn later(now: Timestamp, by: Duration) -> Result<Timestamp> {
    now.checked_add(by)
        .map_err(|error| Error::internal("computing a message time", error))
}
