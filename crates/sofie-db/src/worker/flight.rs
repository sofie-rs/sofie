use std::future::poll_fn;
use std::pin::{Pin, pin};
use std::task::Poll;

use sofie_core::{Result, SqlValue};
use tokio::time::{Instant, sleep};

use super::handling::{self, Handled};
use super::wake::Wake;
use super::{Drained, LEASE, POLL_INTERVAL, UNHEARD_KEPT_FOR, Worker};
use crate::{BoxFuture, Claimed};

pub(super) type Signals<'a> = (Pin<&'a mut (dyn Future<Output = ()> + Send)>, &'a mut Wake);

enum Event {
    Stopped,
    Finished(usize),
    Claimed(Result<Vec<Claimed>>),
    Due,
}

/// Without `signals` it drains: it ends once a claim finds nothing.
pub(super) async fn work(worker: &Worker, mut signals: Option<Signals<'_>>) -> Result<Drained> {
    let mine = SqlValue::Json(worker.registry.known().to_json());
    let mut running: Vec<BoxFuture<'_, Handled>> = Vec::new();
    let mut claiming: Option<BoxFuture<'_, Result<Vec<Claimed>>>> = None;
    let mut poll = pin!(sleep(POLL_INTERVAL));
    let (mut more, mut stopped, mut asked, mut finished_meanwhile) = (true, false, 0, false);
    let mut failures = Vec::new();
    let (mut drained, draining) = (Drained::default(), signals.is_none());
    loop {
        if more && !stopped && claiming.is_none() && running.len() < worker.in_flight {
            (asked, finished_meanwhile) = (worker.in_flight - running.len(), false);
            claiming = Some(Box::pin(claim(worker, &mine, asked, draining)));
        }
        let idle = running.is_empty() && claiming.is_none();
        if idle && (stopped || (draining && !more)) {
            return Ok(drained);
        }
        let (mut shutdown, mut notified) = match signals.as_mut().filter(|_| !stopped) {
            Some((shutdown, wake)) => (Some(shutdown.as_mut()), Some(Box::pin(wake.notified()))),
            None => (None, None),
        };
        let event = poll_fn(|context| {
            if shutdown
                .as_mut()
                .is_some_and(|shutdown| shutdown.as_mut().poll(context).is_ready())
            {
                return Poll::Ready(Event::Stopped);
            }
            let (before, mut jobs) = (running.len(), 0);
            running.retain_mut(|handling| {
                let Poll::Ready(handled) = handling.as_mut().poll(context) else {
                    return true;
                };
                match handled {
                    Handled::Completed => jobs += 1,
                    Handled::Failed(failure) if draining => failures.push(failure),
                    Handled::Delivered | Handled::Failed(_) => {}
                }
                false
            });
            if running.len() < before {
                return Poll::Ready(Event::Finished(jobs));
            }
            if let Some(Poll::Ready(claimed)) =
                claiming.as_mut().map(|claim| claim.as_mut().poll(context))
            {
                claiming = None;
                return Poll::Ready(Event::Claimed(claimed));
            }
            let Some(notified) = notified.as_mut() else {
                return Poll::Pending;
            };
            if notified.as_mut().poll(context).is_pending()
                && poll.as_mut().poll(context).is_pending()
            {
                return Poll::Pending;
            }
            poll.as_mut().reset(Instant::now() + POLL_INTERVAL);
            Poll::Ready(Event::Due)
        });
        match event.await {
            Event::Stopped => stopped = true,
            Event::Finished(jobs) => {
                drained.completed += jobs;
                drained.failures.append(&mut failures);
                (more, finished_meanwhile) = (true, claiming.is_some());
            }
            // A message that finished during the claim may have added jobs
            // the claim did not see.
            Event::Claimed(Ok(claimed)) => {
                more = claimed.len() >= asked || finished_meanwhile;
                let handling = claimed
                    .into_iter()
                    .map(|message| handling::process(worker, message));
                running.extend(handling.map(|future| Box::pin(future) as BoxFuture<'_, Handled>));
            }
            Event::Claimed(Err(error)) if draining => return Err(error),
            Event::Claimed(Err(error)) => {
                tracing::error!(error.kind = "worker", error = %error, "claiming messages failed");
                more = false;
            }
            Event::Due => more = true,
        }
    }
}

/// Draining looks for every local schedule due, so a test that moved the clock sees it fire.
#[cfg_attr(not(feature = "local-time"), expect(unused_variables))]
async fn claim(
    worker: &Worker,
    mine: &SqlValue,
    limit: usize,
    draining: bool,
) -> Result<Vec<Claimed>> {
    let now = worker.db.clock().now().timestamp();
    worker
        .registry
        .schedules
        .fire_due(&worker.messages, now)
        .await?;
    #[cfg(feature = "local-time")]
    if draining || worker.local_looks.due(now) {
        crate::contracts::fire_due(&worker.db, now).await?;
    }
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let held_until = now.checked_sub(UNHEARD_KEPT_FOR).map_err(|error| {
        sofie_core::Error::internal("computing when unhandled messages stop waiting", error)
    })?;
    crate::messages::claim_known(&worker.db, LEASE, limit, mine, held_until).await
}
