use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use sofie_core::{Error, Instant, Result};

use crate::worker::JobContext;
use crate::{Call, Db, MessageKind, Outcome, Plan, Resolve, Writes};

#[derive(Clone, Default)]
pub(crate) struct JobNames(HashMap<TypeId, &'static str>);

pub fn name_job<Args, J: Call<JobContext, Args>>(db: &Db, name: &'static str, _job: J) {
    db.provided.change(|names: &mut JobNames| {
        names.0.insert(TypeId::of::<J>(), name);
    });
}

pub struct Jobs(pub(crate) Arc<JobNames>);

impl<C: Writes> Resolve<C> for Jobs {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        let names = context.db().provided.get::<JobNames>().unwrap_or_default();
        std::future::ready(Ok(Self(names)))
    }
}

impl Jobs {
    /// Stored when the handler's outcome commits.
    pub fn queue<Args, J: Call<JobContext, Args>>(
        &self,
        job: J,
        payload: &impl Serialize,
    ) -> Result<Queued> {
        Queued::new(self.name(job)?, payload, None)
    }

    /// Run no sooner than `at`; one already past runs at once.
    pub fn queue_at<Args, J: Call<JobContext, Args>>(
        &self,
        job: J,
        payload: &impl Serialize,
        at: Instant,
    ) -> Result<Queued> {
        Queued::new(self.name(job)?, payload, Some(at))
    }

    /// For a package's job that queues itself again, which its own type cannot name.
    #[doc(hidden)]
    pub fn queue_named_at(
        name: &'static str,
        payload: &impl Serialize,
        at: Instant,
    ) -> Result<Queued> {
        Queued::new(name, payload, Some(at))
    }

    fn name<Args, J: Call<JobContext, Args>>(&self, _job: J) -> Result<&'static str> {
        self.0.0.get(&TypeId::of::<J>()).copied().ok_or_else(|| {
            Error::internal_message(format!(
                "{} is not a job; mark it `#[job]` and run `sofie gen`",
                std::any::type_name::<J>()
            ))
        })
    }
}

pub struct Queued {
    pub(super) name: &'static str,
    pub(super) payload: Value,
    at: Option<Instant>,
}

impl Queued {
    fn new(name: &'static str, payload: &impl Serialize, at: Option<Instant>) -> Result<Self> {
        let payload = serde_json::to_value(payload)
            .map_err(|error| Error::internal(format!("serializing job `{name}`"), error))?;
        Ok(Self { name, payload, at })
    }
}

impl Outcome for Queued {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        let message = [(self.name, self.payload.clone())];
        match self.at {
            Some(at) => plan.queue_at(MessageKind::Job, message, at.timestamp()),
            None => plan.queue(MessageKind::Job, message),
        }
    }
}
