use sofie_core::Result;

use crate::arrivals::Raised;
use crate::outcome::explain_duplicate;
use crate::{Db, Outcome, Plan, Reach, Step};

pub(crate) async fn commit(
    db: &Db,
    outcome: &mut dyn Outcome,
    steps: Vec<Step>,
    reach: Reach,
) -> Result<()> {
    let heard = db.arrivals().heard().known();
    let mut plan = Plan::new(db.driver().dialect(), reach, db.clock().now(), steps, heard);
    #[cfg(feature = "search-engine")]
    {
        plan.indexes_searches = db.search.get().is_some();
    }
    outcome.plan(&mut plan)?;
    let stamp = plan.stamp();
    let raised = db.arrivals().copy_raised(&plan.events);
    let planned = plan.into_parts()?;
    let mut announcing = Announcing {
        db,
        committed: Some(Committed {
            queued: planned.queues_messages,
            raised,
            announcements: planned.announcements,
        }),
    };
    if !planned.steps.is_empty() {
        let committed = db.transaction(planned.steps).await;
        planned.guard_checks.iter().for_each(|check| check.audit());
        if let Err(error) = committed {
            announcing.committed = None;
            return Err(explain_duplicate(error, &planned.unique_fields));
        }
    }
    drop(announcing);
    outcome.committed(stamp);
    Ok(())
}

#[derive(Default)]
pub(crate) struct Announcements {
    #[cfg(feature = "cache")]
    pub(crate) changed: Vec<crate::ChangedRecord>,
    #[cfg(feature = "live")]
    pub(crate) live: Vec<crate::TableChange>,
}

impl Announcements {
    #[cfg(feature = "cache")]
    pub(crate) fn changed(&mut self, record: crate::ChangedRecord) {
        self.changed.push(record);
    }

    #[cfg(feature = "live")]
    pub(crate) fn live_change(&mut self, change: crate::TableChange) {
        self.live.push(change);
    }

    #[cfg_attr(
        not(any(feature = "cache", feature = "live")),
        expect(unused_variables, clippy::unused_self)
    )]
    fn announce(self, db: &Db) {
        #[cfg(feature = "cache")]
        db.commit_hooks.run(&self.changed);
        #[cfg(feature = "live")]
        db.changes.announce(&db.driver, self.live);
    }
}

struct Committed {
    queued: bool,
    raised: Raised,
    announcements: Announcements,
}

/// Announces on drop too: a caller dropped once COMMIT was sent would otherwise leave a commit that
/// stands unannounced, while announcing one that never landed only wakes readers for nothing.
struct Announcing<'a> {
    db: &'a Db,
    committed: Option<Committed>,
}

impl Drop for Announcing<'_> {
    fn drop(&mut self) {
        let Some(committed) = self.committed.take() else {
            return;
        };
        let db = self.db;
        db.arrivals()
            .committed(&db.driver, committed.queued, committed.raised);
        committed.announcements.announce(db);
    }
}
