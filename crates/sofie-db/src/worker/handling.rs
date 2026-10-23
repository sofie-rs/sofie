use sofie_core::{Error, Result};
use tracing::Instrument;

use super::backoff::settle;
#[cfg(feature = "metrics")]
use super::metrics;
use super::trace::{self, Work};
use super::{Failure, JobContext, Worker, lease};
use crate::heard::Listeners;
use crate::messages::keyed_job;
use crate::{Claimed, MessageKind, Trigger};

pub(super) enum Handled {
    Completed,
    Delivered,
    Failed(Failure),
}

pub(super) async fn process(worker: &Worker, message: Claimed) -> Handled {
    let heard = worker.db.arrivals().heard().known().unwrap_or_default();
    let listener = worker.registry.only_listener(&message, &heard.listeners);
    let work = worker.registry.work_of(&message, listener);
    let name = listener.unwrap_or(&message.name);
    let span = trace::message_span(&message, work, name);
    let handling = handle(worker, &message, listener, &heard.listeners).instrument(span.clone());
    let outcome = lease::held(&worker.messages, &message, handling).await;
    #[cfg(feature = "metrics")]
    metrics::measure(worker, &message, work, name, outcome.as_ref().err());
    let handled = match &outcome {
        Ok(()) if matches!(work, Work::Event) => Handled::Delivered,
        Ok(()) => Handled::Completed,
        Err(error) => Handled::Failed(Failure {
            job: name.to_owned(),
            error: error.to_string(),
        }),
    };
    settle(worker, &message, listener, outcome)
        .instrument(span)
        .await;
    handled
}

/// An event queued before any worker was published goes to every listener the process heard.
async fn handle(
    worker: &Worker,
    message: &Claimed,
    only_listener: Option<&'static str>,
    heard: &Listeners,
) -> Result<()> {
    // Claiming counts attempts past the last for a worker that died.
    if message.attempt > worker.max_attempts {
        return Err(Error::internal_message(
            "its worker stopped during its last attempt",
        ));
    }
    match (message.kind, only_listener) {
        (MessageKind::Event, Some(listener)) => run_job(worker, listener, message).await,
        (MessageKind::Event, None) => {
            if heard.of(&message.name).is_empty() {
                tracing::warn!(
                    event.name = %message.name,
                    message.id = %message.id,
                    "no worker took an event before its wait ran out; it is dropped"
                );
            }
            let listeners = heard.taking(&message.name, &message.payload);
            fan_out(worker, message, &listeners).await
        }
        (MessageKind::Job, _) => run_job(worker, &message.name, message).await,
    }
}

/// Due when the event was, so under a backlog they run before later events.
async fn fan_out(worker: &Worker, event: &Claimed, listeners: &[&str]) -> Result<()> {
    let dialect = worker.db.driver().dialect();
    let mut steps = vec![worker.messages.completion(event)];
    for listener in listeners {
        let payload = event.payload.clone();
        steps.push(keyed_job(
            dialect,
            listener,
            payload,
            event.id,
            event.available_at,
            event.actor,
        )?);
    }
    worker.db.transaction(steps).await
}

async fn run_job(worker: &Worker, name: &str, message: &Claimed) -> Result<()> {
    let job = worker
        .registry
        .job(name)
        .ok_or_else(|| Error::internal_message(format!("no job named `{name}` is registered")))?;
    let completion = worker.messages.completion(message);
    let event = match job.trigger {
        Trigger::On(event) => Some(event),
        Trigger::Queued | Trigger::Every(_) => None,
    };
    let mut context = JobContext::new(worker, message, event, completion);
    let run = &job.run;
    let work = async move {
        let mut outcome = run(&mut context).await?;
        context.complete(&mut *outcome).await
    };
    let work = crate::action::acting_for(job.name, work);
    lease::caught(sofie_core::moves::recorded(work)).await
}
