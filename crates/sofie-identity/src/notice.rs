use sofie::db::{Event, Outcome, Plan};
use sofie::prelude::*;

/// An event committed on its own, with no record changed.
pub(crate) struct Notice(Meta);

impl Notice {
    pub(crate) fn of<E: Event>(event: E) -> Result<Self> {
        let mut meta = Meta::new()?;
        meta.raise(event)?;
        Ok(Self(meta))
    }
}

impl Outcome for Notice {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        plan.raise(self.0.events());
        Ok(())
    }
}
