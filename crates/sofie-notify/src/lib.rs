//! One way to notify a principal: in its inbox, by mail and by web push, held to its
//! choices, quiet hours and digest, and delivered by jobs queued with the handler's commit.

use sofie_db::{Context, Db, Dep, Jobs, Queued, Resolve, Writes};
mod deliver;
mod digest;
mod inbox;
mod notice;
mod preferences;
mod push;
mod retention;
mod store;
mod time_of_day;
mod topic;

use std::sync::Arc;

use serde::de::DeserializeOwned;
use sofie_client::Outgoing;
use sofie_core::{Error, Result};
use sofie_db::worker::{JobContext, Worker};

pub use inbox::{Inbox, InboxItem, MarkedRead};
pub use jiff::civil::Weekday;
pub use notice::{Notice, Recipient};
pub use preferences::{Changed, ChannelChoice, Chosen, Digest, Preferences};
pub use push::{
    PushEndpoint, PushRegistration, SERVICE_WORKER, Subscribed, Unsubscribed, VapidKey, VapidKeys,
    subscribe_push, unsubscribe_push, vapid_key,
};
pub use store::personal_data;
pub use time_of_day::TimeOfDay;
pub use topic::{Channel, Topic};

/// Without VAPID keys the push channel is skipped.
#[derive(Clone)]
pub struct Setup {
    pub(crate) vapid: Option<Arc<VapidKeys>>,
    /// Untrusted: each browser chose its push service.
    pub(crate) http: Outgoing,
}

impl Setup {
    pub fn new(vapid: Option<VapidKeys>) -> Result<Self> {
        Ok(Self {
            vapid: vapid.map(Arc::new),
            http: sofie_client::for_vendor("web_push", sofie_client::Vendor::default())?,
        })
    }

    /// `VAPID_PUBLIC_KEY`, `VAPID_PRIVATE_KEY` (base64url) and `VAPID_SUBJECT`, all or none.
    #[doc(hidden)]
    pub fn from_environment() -> Result<Self> {
        let keys = ["PUBLIC_KEY", "PRIVATE_KEY", "SUBJECT"].map(variable);
        let vapid = match keys {
            [Ok(Some(public)), Ok(Some(private)), Ok(Some(subject))] => {
                Some(VapidKeys::new(&public, &private, &subject)?)
            }
            [Ok(None), Ok(None), Ok(None)] => None,
            [public, private, subject] => {
                public?;
                private?;
                subject?;
                return Err(Error::internal_message(
                    "set all of SOFIE_NOTIFY_VAPID_PUBLIC_KEY, _PRIVATE_KEY and _SUBJECT, or none",
                ));
            }
        };
        Self::new(vapid)
    }
}

fn variable(name: &str) -> Result<Option<String>> {
    let key = format!("SOFIE_NOTIFY_VAPID_{name}");
    Ok(sofie_core::environment_variable(&key)?.filter(|value| !value.trim().is_empty()))
}

#[doc(hidden)]
pub fn install(db: &Db, setup: &Setup) {
    sofie_db::provide(db, Arc::new(setup.clone()));
    sofie_db::name_job(db, deliver::JOB, deliver::deliver);
    sofie_db::name_job(db, push::JOB, push::push);
    sofie_db::name_job(db, digest::JOB, digest::send_digest);
}

#[doc(hidden)]
#[must_use]
pub fn worker(worker: Worker) -> Worker {
    worker
        .job(deliver::JOB, deliver::deliver)
        .job(push::JOB, push::push)
        .job(digest::JOB, digest::send_digest)
        .every(retention::JOB, retention::EVERY, retention::prune)
}

/// Sends notifications from a handler, job or task; each is delivered once the outcome commits.
pub struct Notify {
    setup: Dep<Setup>,
    jobs: Jobs,
}

impl<C: Writes> Resolve<C> for Notify {
    async fn resolve(context: &mut C) -> Result<Self> {
        let setup = Dep::<Setup>::resolve(context).await.map_err(|_| {
            Error::internal_message(
                "notifications are not set up: `sofie::run` sets them up with sofie's `notify` \
                 feature, and a test with `sofie_notify::install`",
            )
        })?;
        Ok(Self {
            setup,
            jobs: Jobs::resolve(context).await?,
        })
    }
}

impl Notify {
    pub fn send(&self, recipient: Recipient, topic: Topic, notice: Notice) -> Result<Queued> {
        let sending = deliver::Sending {
            recipient,
            topic: topic.name.to_owned(),
            defaults: topic.defaults.to_vec(),
            urgent: topic.urgent,
            digested: topic.digested,
            notice,
            held: None,
        };
        self.jobs.queue(deliver::deliver, &sending)
    }

    #[must_use]
    pub fn vapid_key(&self) -> Option<&str> {
        self.setup.vapid.as_deref().map(VapidKeys::public_key)
    }
}

fn job_input<T: DeserializeOwned>(context: &mut JobContext, job: &str) -> Result<T> {
    serde_json::from_value(context.input()?)
        .map_err(|error| Error::internal(format!("reading {job}'s input"), error))
}
