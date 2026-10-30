use std::sync::Arc;

use sofie_auth::SigningKeys;
use sofie_core::{Clock, Error, Result};
use tracing::Instrument;

use crate::arrivals::{Arrivals, Raised};
use crate::outcome::{Planned, explain_duplicate};
use crate::{Driver, Outcome, Plan, Reach, Read, Schema, Step};

#[derive(Clone)]
pub struct Db {
    pub(crate) driver: Arc<dyn Driver>,
    clock: Clock,
    arrivals: Arc<Arrivals>,
    keys: Option<Arc<SigningKeys>>,
    #[cfg(feature = "contracts")]
    pub(crate) provided: Arc<crate::contracts::Provided>,
    #[cfg(feature = "cache")]
    pub(crate) commit_hooks: Arc<crate::commit_hooks::CommitHooks>,
    #[cfg(feature = "live")]
    pub(crate) changes: Arc<crate::live::Changes>,
    #[cfg(feature = "search-engine")]
    pub(crate) search: Arc<std::sync::OnceLock<crate::search::Installed>>,
}

impl Db {
    pub fn new(driver: impl Driver) -> Self {
        #[cfg(feature = "metrics")]
        let driver = crate::metrics::Counted::new(driver);
        let driver: Arc<dyn Driver> = Arc::new(driver);
        Self {
            #[cfg(feature = "live")]
            changes: Arc::new(crate::live::Changes::new(&driver)),
            driver,
            clock: Clock::system(),
            arrivals: Arc::default(),
            keys: None,
            #[cfg(feature = "contracts")]
            provided: Arc::default(),
            #[cfg(feature = "cache")]
            commit_hooks: Arc::default(),
            #[cfg(feature = "search-engine")]
            search: Arc::default(),
        }
    }

    #[must_use]
    pub fn with_clock(self, clock: Clock) -> Self {
        Self { clock, ..self }
    }

    /// Wakes, hooks and screens of `other` then see this one's commits as their own.
    #[must_use]
    pub fn sharing_arrivals(self, other: &Db) -> Self {
        Self {
            arrivals: Arc::clone(&other.arrivals),
            #[cfg(feature = "cache")]
            commit_hooks: Arc::clone(&other.commit_hooks),
            #[cfg(feature = "live")]
            changes: Arc::clone(&other.changes),
            ..self
        }
    }

    #[must_use]
    pub fn with_signing_keys(self, keys: Arc<SigningKeys>) -> Self {
        Self {
            keys: Some(keys),
            ..self
        }
    }

    pub(crate) fn signing_keys(&self) -> Option<Arc<SigningKeys>> {
        self.keys.clone()
    }

    #[must_use]
    pub fn clock(&self) -> Clock {
        self.clock.clone()
    }

    #[must_use]
    pub fn read(&self) -> Read {
        Read::within(self, Reach::All)
    }

    #[must_use]
    pub fn schema(&self) -> Schema<'_> {
        Schema { db: self }
    }

    pub(crate) fn arrivals(&self) -> &Arrivals {
        &self.arrivals
    }

    pub(crate) fn driver(&self) -> &dyn Driver {
        self.driver.as_ref()
    }

    /// Opens no transaction when there is nothing to write.
    pub async fn commit(&self, outcome: &mut dyn Outcome) -> Result<()> {
        self.commit_with(outcome, Vec::new(), Reach::All).await
    }

    /// `steps` run first; nothing is written when it would reach a record beyond `reach`.
    pub async fn commit_with(
        &self,
        outcome: &mut dyn Outcome,
        steps: Vec<Step>,
        reach: Reach,
    ) -> Result<()> {
        let now = self.clock.now();
        let heard = self.arrivals.heard().known();
        let mut plan = Plan::new(self.driver().dialect(), reach, now, steps, heard);
        #[cfg(feature = "search-engine")]
        if self.search.get().is_some() {
            plan.index_searches();
        }
        outcome.plan(&mut plan)?;
        let stamp = plan.stamp();
        let raised = self.arrivals.copy_raised(&plan.events);
        let mut planned = plan.into_parts()?;
        let unique_fields = std::mem::take(&mut planned.unique_fields);
        let db = self.clone();
        // A caller dropped once COMMIT is sent would otherwise skip announcing a commit that stands.
        let committing = async move { db.commit_planned(planned, raised).await };
        tokio::spawn(committing.in_current_span())
            .await
            .map_err(|error| Error::internal("committing a transaction", error))?
            .map_err(|error| explain_duplicate(error, &unique_fields))?;
        outcome.committed(stamp);
        Ok(())
    }

    async fn commit_planned(&self, planned: Planned, raised: Raised) -> Result<()> {
        if !planned.steps.is_empty() {
            let committed = self.transaction(planned.steps).await;
            planned.guard_checks.iter().for_each(|check| check.audit());
            committed?;
        }
        let queued = planned.queues_messages;
        self.arrivals.committed(&self.driver, queued, raised);
        #[cfg(feature = "cache")]
        self.commit_hooks.run(&planned.changed);
        #[cfg(feature = "live")]
        self.changes.announce(&self.driver, planned.live);
        Ok(())
    }

    pub async fn ping(&self) -> Result<()> {
        crate::query_values(self.driver(), "SELECT 1", &[])
            .await
            .map(drop)
    }

    pub(crate) async fn transaction(&self, steps: Vec<Step>) -> Result<()> {
        let span = tracing::info_span!("commit", db.statements = steps.len());
        self.driver.transaction(steps).instrument(span).await
    }
}
