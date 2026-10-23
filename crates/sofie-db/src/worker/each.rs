use std::sync::Arc;

use serde_json::{Value, json};
use sofie_core::{Result, Uuid};

use super::JobContext;
use crate::messages::keyed_job;
use crate::{Aggregate, AnyOutcome, Call, Outcome, Plan, Run};

pub(super) fn each<A: Aggregate, Args: 'static>(
    items: impl Call<JobContext, Args, Outcome = Vec<A>>,
    action: &'static str,
) -> Run<JobContext> {
    Arc::new(move |context| {
        let items = items.clone();
        Box::pin(async move {
            let records = items.call(context).await?;
            let jobs = records
                .iter()
                .map(|record| (json!({ A::NAME: record.id() }), record.id().uuid()))
                .collect();
            Ok(Box::new(EachQueued { action, jobs }) as Box<dyn AnyOutcome>)
        })
    })
}

/// A record whose job is still waiting gets no second one.
struct EachQueued {
    action: &'static str,
    jobs: Vec<(Value, Uuid)>,
}

impl Outcome for EachQueued {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        let stamp = plan.stamp();
        let now = stamp.now.timestamp();
        for (payload, key) in &self.jobs {
            let payload = payload.clone();
            let step = keyed_job(plan.dialect(), self.action, payload, *key, now, stamp.actor)?;
            plan.push_message(step);
        }
        Ok(())
    }
}
