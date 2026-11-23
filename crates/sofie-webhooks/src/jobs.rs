use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sofie::auth::{SigningKeys, WEBHOOK_ID, WEBHOOK_SIGNATURE, WEBHOOK_TIMESTAMP, WebhookSecret};
use sofie::core::Uuid;
use sofie::db::worker::JobContext;
use sofie::db::{Context, Outcome, Plan, Queued, Resolve, Stamp};
use sofie::prelude::*;

use crate::delivery::{Attempt, Next, WebhookDelivery};
use crate::endpoint::{Disabled, WebhookEndpoint};
use crate::sender::Transport;

pub(crate) const FAN_OUT: &str = "sofie_webhooks.fan_out";
pub(crate) const DELIVER: &str = "sofie_webhooks.deliver";
const USER_AGENT: &str = "sofie-webhooks";
/// A final failure after this long without a success turns the endpoint off.
const FAILING_FOR: Duration = Duration::from_hours(24 * 5);
/// Its last success is written at most this often, so busy endpoints are not rewritten on
/// every delivery and their owner's edits rarely meet a conflict.
const SUCCESS_RECORDED_EVERY: Duration = Duration::from_hours(1);

#[derive(Serialize, Deserialize)]
pub(crate) struct FanningOut {
    pub(crate) owner: Uuid,
    pub(crate) event: String,
    pub(crate) body: String,
}

impl Resolve<JobContext> for FanningOut {
    fn resolve(context: &mut JobContext) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(input(context, FAN_OUT))
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Delivering {
    pub(crate) delivery: Uuid,
}

impl Resolve<JobContext> for Delivering {
    fn resolve(context: &mut JobContext) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(input(context, DELIVER))
    }
}

fn input<T: serde::de::DeserializeOwned>(context: &mut JobContext, job: &str) -> Result<T> {
    sofie::serde_json::from_value(context.input()?)
        .map_err(|error| Error::internal(format!("reading {job}'s input"), error))
}

pub(crate) struct Keys(pub(crate) Arc<SigningKeys>);

impl Resolve<JobContext> for Keys {
    fn resolve(context: &mut JobContext) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(context.signing_keys().map(Self))
    }
}

pub(crate) fn queue_delivery(delivery: &WebhookDelivery, at: Instant) -> Result<Queued> {
    let delivering = Delivering {
        delivery: delivery.id().uuid(),
    };
    Jobs::queue_named_at(DELIVER, &delivering, at)
}

pub(crate) async fn fan_out(
    fanning_out: FanningOut,
    read: Read,
    clock: Clock,
) -> Result<Deliveries> {
    let now = clock.now();
    let endpoints = WebhookEndpoint::query()
        .where_(WebhookEndpoint::OWNER_ID.eq(Id::from_uuid(fanning_out.owner)))
        .all(&read)
        .await?;
    let mut deliveries = Deliveries::default();
    for endpoint in endpoints
        .iter()
        .filter(|endpoint| endpoint.subscribes_to(&fanning_out.event))
    {
        let delivery = WebhookDelivery::new(
            fanning_out.owner,
            endpoint.id(),
            &fanning_out.event,
            fanning_out.body.clone(),
        )?;
        deliveries.queued.push(queue_delivery(&delivery, now)?);
        deliveries.made.push(delivery);
    }
    Ok(deliveries)
}

#[derive(Default)]
pub(crate) struct Deliveries {
    pub(crate) made: Vec<WebhookDelivery>,
    pub(crate) queued: Vec<Queued>,
}

impl Outcome for Deliveries {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.made.plan(plan)?;
        self.queued.plan(plan)
    }

    fn committed(&mut self, stamp: Stamp) {
        self.made.committed(stamp);
    }
}

pub(crate) async fn deliver(
    delivering: Delivering,
    read: Read,
    clock: Clock,
    keys: Keys,
    transport: Transport,
) -> Result<Attempted> {
    let delivery = WebhookDelivery::query()
        .where_(WebhookDelivery::ID.eq(Id::from_uuid(delivering.delivery)))
        .first(&read)
        .await?;
    let Some(mut delivery) = delivery.filter(|delivery| !delivery.is_settled()) else {
        return Ok(Attempted::default());
    };
    let now = clock.now();
    let endpoint = WebhookEndpoint::query()
        .where_(WebhookEndpoint::ID.eq(delivery.endpoint()))
        .first(&read)
        .await?;
    let Some(mut endpoint) = endpoint.filter(|endpoint| endpoint.disabled_at().is_none()) else {
        delivery.give_up(now);
        return Ok(Attempted {
            delivery: Some(delivery),
            ..Attempted::default()
        });
    };
    let headers = signed_headers(&delivery, &endpoint, &keys.0, now)?;
    let sent = transport
        .0
        .post(endpoint.url(), &headers, delivery.body())
        .await;
    let attempt = attempt_of(sent);
    let delivered = matches!(attempt, Attempt::Delivered(_));
    let mut attempted = Attempted::default();
    let mut endpoint_changed = false;
    match delivery.attempted(attempt, now)? {
        Next::RetryAt(at) => attempted.next = Some(queue_delivery(&delivery, at)?),
        Next::GaveUp
            if endpoint
                .failing_since()
                .is_some_and(|since| elapsed(since, now) > FAILING_FOR) =>
        {
            endpoint.disable(Disabled::Failing, now);
            tracing::warn!(
                webhook.endpoint = %endpoint.id(),
                webhook.owner = %endpoint.owner(),
                "a webhook endpoint failed for days and was turned off"
            );
            endpoint_changed = true;
        }
        Next::Done | Next::GaveUp => {}
    }
    if delivered
        && endpoint
            .succeeded_at()
            .is_none_or(|at| elapsed(at, now) >= SUCCESS_RECORDED_EVERY)
    {
        endpoint.succeeded(now);
        endpoint_changed = true;
    }
    attempted.delivery = Some(delivery);
    attempted.endpoint = endpoint_changed.then_some(endpoint);
    Ok(attempted)
}

fn elapsed(since: Instant, now: Instant) -> Duration {
    let seconds = now
        .timestamp()
        .as_second()
        .saturating_sub(since.timestamp().as_second());
    Duration::from_secs(u64::try_from(seconds).unwrap_or(0))
}

fn signed_headers(
    delivery: &WebhookDelivery,
    endpoint: &WebhookEndpoint,
    keys: &SigningKeys,
    now: Instant,
) -> Result<Vec<(&'static str, String)>> {
    let id = delivery.id().to_string();
    let timestamp = now.timestamp().as_second();
    let signatures = endpoint
        .sealed_secrets(now)
        .map(|sealed| {
            let plain = keys
                .open_value(crate::SECRET_PURPOSE, sealed)
                .and_then(|plain| String::from_utf8(plain).ok())
                .ok_or_else(|| {
                    Error::internal_message(
                        "a webhook secret does not open with the app's signing keys",
                    )
                })?;
            Ok(WebhookSecret::parse(&plain)?.sign(&id, timestamp, delivery.body().as_bytes()))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(vec![
        (WEBHOOK_ID, id),
        (WEBHOOK_TIMESTAMP, timestamp.to_string()),
        (WEBHOOK_SIGNATURE, signatures.join(" ")),
        ("user-agent", USER_AGENT.to_owned()),
    ])
}

/// A refused URL refuses every later attempt too; anything else unanswered may pass later.
fn attempt_of(sent: Result<u16>) -> Attempt {
    match sent {
        Ok(status @ 200..=299) => Attempt::Delivered(status),
        Ok(status) => Attempt::Refused(status),
        Err(Error::BadRequest(reason)) => Attempt::Unanswered {
            error: reason,
            retry: false,
        },
        Err(error) => Attempt::Unanswered {
            error: error.to_string(),
            retry: true,
        },
    }
}

#[derive(Default)]
pub(crate) struct Attempted {
    delivery: Option<WebhookDelivery>,
    endpoint: Option<WebhookEndpoint>,
    next: Option<Queued>,
}

impl Outcome for Attempted {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.delivery.plan(plan)?;
        self.endpoint.plan(plan)?;
        self.next.plan(plan)
    }

    fn committed(&mut self, stamp: Stamp) {
        self.delivery.committed(stamp);
        self.endpoint.committed(stamp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_url_is_given_up_and_a_lost_connection_retried() {
        assert_eq!(attempt_of(Ok(204)), Attempt::Delivered(204));
        assert_eq!(attempt_of(Ok(410)), Attempt::Refused(410));
        assert!(matches!(
            attempt_of(Err(Error::BadRequest("private address".to_owned()))),
            Attempt::Unanswered { retry: false, .. }
        ));
        assert!(matches!(
            attempt_of(Err(Error::Unavailable("timed out".to_owned()))),
            Attempt::Unanswered { retry: true, .. }
        ));
    }
}
