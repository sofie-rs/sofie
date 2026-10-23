use sofie_core::Result;
use sofie_db::{Call, Outcome, Plan, Reply, Stamp};

use crate::{Feature, Flags, Subject};

/// Acts for no principal, so a flag only some principals have is off for it.
pub fn gated<H>(flags: &Flags, feature: Feature, handler: H) -> Gated<H> {
    Gated {
        flags: flags.clone(),
        feature,
        handler,
    }
}

#[derive(Clone)]
pub struct Gated<H> {
    flags: Flags,
    feature: Feature,
    handler: H,
}

impl<C: Send, Args, H: Call<C, Args>> Call<C, Args> for Gated<H> {
    type Outcome = Skippable<H::Outcome>;

    async fn call(&self, context: &mut C) -> Result<Self::Outcome> {
        let features = self.flags.evaluate(Subject::default());
        if !features.enabled(self.feature) {
            let name = self.feature.name();
            tracing::info!(feature_flag.key = name, "skipped: its feature is off");
            return Ok(Skippable(None));
        }
        sofie_telemetry::attach_feature_flags(&tracing::Span::current(), move || {
            features.evaluated()
        });
        self.handler
            .call(context)
            .await
            .map(|outcome| Skippable(Some(outcome)))
    }
}

pub struct Skippable<O>(Option<O>);

impl<O: Outcome> Outcome for Skippable<O> {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.0.as_ref().map_or(Ok(()), |outcome| outcome.plan(plan))
    }

    fn committed(&mut self, stamp: Stamp) {
        if let Some(outcome) = &mut self.0 {
            outcome.committed(stamp);
        }
    }

    fn reply(self) -> Result<Reply> {
        self.0.map_or_else(|| ().reply(), Outcome::reply)
    }
}
