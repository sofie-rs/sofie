//! Outgoing webhooks: an owner registers endpoints for the events an area offers, and every
//! event sent for that owner reaches each subscribed endpoint once, signed as Standard
//! Webhooks and retried for about a day. A delivery is queued with the handler's commit, so
//! an event is sent only for what was written.

mod delivery;
mod endpoint;
mod jobs;
mod retention;
mod routes;
mod secret;
mod sender;

use serde::Serialize;
use sofie::core::Uuid;
use sofie::db::worker::Worker;
use sofie::db::{Migrations, Queued, Resolve, Writes};
use sofie::prelude::*;

pub use delivery::WebhookDelivery;
pub use endpoint::{Disabled, WebhookEndpoint, WebhookEndpointId};
pub use routes::{ROUTES, routes};
pub use sender::WebhookSender;

const MODULE_NAME: &str = "sofie_webhooks";
/// Receivers keep bodies small; a larger event names its record for the receiver to fetch.
const MAXIMUM_BODY: usize = 256 * 1024;

pub const MODULE: sofie::http::Module = sofie::http::Module {
    migrations: Some(migrations()),
    jobs: worker,
    ..sofie::http::Module::new(MODULE_NAME)
};

/// An event an area's principals may subscribe to, named `<thing>.<happened>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WebhookEvent {
    pub name: &'static str,
    /// Shown to owners choosing what to hear.
    pub summary: &'static str,
}

impl WebhookEvent {
    #[must_use]
    pub const fn new(name: &'static str, summary: &'static str) -> Self {
        Self { name, summary }
    }
}

pub trait WebhookArea: Area {
    const WEBHOOK_EVENTS: &'static [WebhookEvent];
}

#[must_use]
pub fn worker(worker: Worker) -> Worker {
    worker
        .job(jobs::FAN_OUT, jobs::fan_out)
        .job(jobs::DELIVER, jobs::deliver)
        .every(retention::JOB, retention::EVERY, retention::prune)
}

#[must_use]
pub const fn migrations() -> Migrations {
    Migrations::new(
        MODULE_NAME,
        sofie::db::embed!("migrations", ["0000_module_role.sql", "0001_webhooks.sql"]),
    )
    .sqlite(sofie::db::embed!(
        "migrations/sqlite",
        ["0000_module_role.sql", "0001_webhooks.sql"]
    ))
    .aggregates(AGGREGATES)
}

const AGGREGATES: &[sofie::db::AggregateTable] = &[
    sofie::db::AggregateTable::of::<WebhookEndpoint>(),
    sofie::db::AggregateTable::of::<WebhookDelivery>(),
];

/// Sends events from a handler, job or listener; each is delivered once the outcome commits.
pub struct Webhooks {
    clock: Clock,
}

impl<C: Writes> Resolve<C> for Webhooks {
    async fn resolve(context: &mut C) -> Result<Self> {
        Ok(Self {
            clock: Clock::resolve(context).await?,
        })
    }
}

impl Webhooks {
    pub fn send(&self, owner: Uuid, event: &WebhookEvent, data: &impl Serialize) -> Result<Queued> {
        let data = sofie::serde_json::to_value(data).map_err(|error| {
            Error::internal(format!("serializing webhook `{}`", event.name), error)
        })?;
        let body = sofie::serde_json::json!({
            "type": event.name,
            "timestamp": self.clock.now().timestamp().to_string(),
            "data": data,
        })
        .to_string();
        if body.len() > MAXIMUM_BODY {
            return Err(Error::internal_message(format!(
                "webhook `{}` is {} bytes, past the {MAXIMUM_BODY} receivers are sent; send the \
                 record's id instead",
                event.name,
                body.len()
            )));
        }
        let fanning_out = jobs::FanningOut {
            owner,
            event: event.name.to_owned(),
            body,
        };
        Jobs::queue_named_at(jobs::FAN_OUT, &fanning_out, self.clock.now())
    }
}
