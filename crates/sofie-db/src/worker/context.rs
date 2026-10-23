use std::sync::Arc;

use serde_json::Value;
use sofie_auth::SigningKeys;
use sofie_core::{Error, Result, Uuid};

use crate::{Claimed, Context, Db, Outcome, Reach, Step, Writes};

pub struct JobContext {
    pub(crate) db: Db,
    pub(crate) payload: Value,
    event: Option<&'static str>,
    actor: Option<Uuid>,
    completion: Step,
    #[cfg(feature = "remote-contracts")]
    pub(crate) message: sofie_core::Uuid,
    /// Keyed by the message's id, so a retry gets the same key.
    #[cfg(feature = "idempotency")]
    pub(crate) work: crate::Work,
    #[cfg(feature = "contracts")]
    pub(crate) retried_for: std::time::Duration,
}

impl JobContext {
    pub(super) fn new(
        worker: &super::Worker,
        message: &Claimed,
        event: Option<&'static str>,
        completion: Step,
    ) -> Self {
        Self {
            db: worker.db.clone(),
            payload: message.payload.clone(),
            event,
            actor: message.actor,
            completion,
            #[cfg(feature = "remote-contracts")]
            message: message.id,
            #[cfg(feature = "idempotency")]
            work: crate::Work::new(message.id.to_string(), message.name.clone()),
            #[cfg(feature = "contracts")]
            retried_for: super::backoff::retried_for(worker.max_attempts),
        }
    }

    /// The keys requests sign links with, so a job's links open the same routes.
    pub fn signing_keys(&self) -> Result<Arc<SigningKeys>> {
        self.db.signing_keys().ok_or_else(|| {
            Error::internal_message(
                "the worker's database has no signing keys; give it the app's with \
                 `Db::with_signing_keys`",
            )
        })
    }

    pub(crate) async fn complete(self, outcome: &mut dyn Outcome) -> Result<()> {
        let reach = self.reach();
        self.db
            .commit_with(outcome, vec![self.completion], reach)
            .await
    }
}

/// A job reaches every record, as the principal whose action queued it if one did.
impl Context for JobContext {
    fn db(&self) -> &Db {
        &self.db
    }

    fn reach(&self) -> Reach {
        self.actor.map_or(Reach::All, Reach::AllAs)
    }

    /// The framework wrote the payload, so a missing id is its fault, not a lookup's.
    fn bound(&self, name: &str) -> Result<Uuid> {
        self.payload
            .get(name)
            .and_then(Value::as_str)
            .and_then(|text| text.parse().ok())
            .ok_or_else(|| Error::internal_message(format!("the payload has no id named `{name}`")))
    }

    #[cfg(feature = "jobs")]
    fn input(&mut self) -> Result<Value> {
        Ok(self.payload.clone())
    }

    fn event(&self, name: &str) -> Result<Value> {
        match self.event {
            Some(delivered) if delivered == name => Ok(self.payload.clone()),
            Some(delivered) => Err(Error::internal_message(format!(
                "this listener receives `{delivered}`, so it cannot take a `{name}`"
            ))),
            None => Err(Error::internal_message(format!(
                "a scheduled job receives no event, so it cannot take a `{name}`"
            ))),
        }
    }
}

impl Writes for JobContext {}
