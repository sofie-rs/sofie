use std::time::Duration;

use sofie_core::{SqlValue, Timestamp};
use sofie_db::Event;
use sofie_db::worker::{Drained, Failure};

use crate::TestApp;

pub(crate) async fn advance(app: &TestApp, by: Duration) -> usize {
    let until = now(app)
        .checked_add(by)
        .expect("advancing within the calendar");
    let mut completed = app.drain().await;
    while let Some(due) = next_due(app, until).await {
        move_to(app, due);
        completed += app.drain().await;
    }
    move_to(app, until);
    completed + app.drain().await
}

pub(crate) async fn drain(app: &TestApp) -> usize {
    let drained = drain_reporting(app).await;
    assert!(
        drained.failures.is_empty(),
        "jobs failed while draining: {:?}",
        drained.failures
    );
    drained.completed
}

/// Each failed job is retried once its backoff is over, as `advance` reaches it.
pub async fn drain_failing(app: &TestApp) -> Vec<Failure> {
    drain_reporting(app).await.failures
}

async fn drain_reporting(app: &TestApp) -> Drained {
    app.worker
        .drain_reporting()
        .await
        .expect("draining the worker")
}

/// Every `E` committed, delivered or not; for a test that waits until one is.
#[must_use]
pub fn raised<E: Event>(app: &TestApp) -> Vec<E> {
    sofie_db::raised(app.db(), E::NAME)
        .into_iter()
        .map(|payload| E::from_json(payload).expect("reading a raised event"))
        .collect()
}

pub fn assert_raised<E: Event>(app: &TestApp) -> Vec<E> {
    let raised = raised::<E>(app);
    assert!(!raised.is_empty(), "no `{}` was raised", E::NAME);
    raised
}

pub(crate) fn now(app: &TestApp) -> Timestamp {
    app.clock.clock().now().timestamp()
}

async fn next_due(app: &TestApp, until: Timestamp) -> Option<Timestamp> {
    let rows = app
        .query(
            "SELECT min(due) FROM (
                SELECT next_at AS due FROM sofie.schedules WHERE next_at > $1 AND next_at <= $2
                UNION ALL
                SELECT available_at FROM sofie.messages
                WHERE dead_at IS NULL AND available_at > $1 AND available_at <= $2
            ) AS due",
            &[SqlValue::Timestamp(now(app)), SqlValue::Timestamp(until)],
        )
        .await;
    match rows.first().and_then(|row| row.first()) {
        Some(SqlValue::Timestamp(due)) => Some(*due),
        _ => None,
    }
}

fn move_to(app: &TestApp, moment: Timestamp) {
    let micros = moment.as_microsecond() - now(app).as_microsecond();
    let forward = u64::try_from(micros).expect("the clock only moves forward");
    app.clock
        .advance(Duration::from_micros(forward))
        .expect("moving the test clock");
}
