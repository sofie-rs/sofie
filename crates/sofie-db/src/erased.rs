use sofie_core::{Error, Result};

use crate::{Aggregate, Outcome, Plan};

#[must_use]
pub struct Erased<A> {
    record: A,
}

impl<A: Aggregate> Erased<A> {
    pub(crate) fn new(record: A) -> Self {
        Self { record }
    }
}

impl<A: Aggregate> Outcome for Erased<A> {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        let record = &self.record;
        let meta = record.meta();
        if meta.snapshot.is_none() {
            return Err(Error::internal_message(format!(
                "a `{}` that was never saved cannot be deleted permanently",
                A::NAME
            )));
        }
        #[cfg_attr(not(feature = "children"), expect(unused_variables))]
        let owned_through_parent = crate::unit_of_work::refuse_beyond_reach(record, plan.reach())?;
        #[cfg(feature = "children")]
        if let Some(owner) = owned_through_parent {
            let check =
                crate::children::through::ownership_check::<A>(meta.id(), owner, plan.dialect());
            plan.push_as::<A>(check);
        }
        crate::prune::plan_deletion(std::slice::from_ref(record), plan)
    }
}
