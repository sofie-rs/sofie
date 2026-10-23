mod backoff;
mod context;
mod each;
#[cfg(feature = "remote-contracts")]
mod feeds;
mod flight;
mod handling;
#[cfg(feature = "jobs")]
mod jobs;
mod lease;
#[cfg(feature = "metrics")]
mod metrics;
mod registry;
mod schedules;
mod trace;
mod wake;
#[cfg(feature = "local-time")]
mod wall_clock;

use std::num::{NonZeroU16, NonZeroUsize};
use std::pin::pin;
use std::time::Duration;

use sofie_core::Result;

pub use context::JobContext;
#[cfg(feature = "remote-contracts")]
pub use feeds::Feeds;
pub use registry::publish_heard;
pub use schedules::{Cadence, queue_schedule};
#[cfg(feature = "local-time")]
pub use wall_clock::WallClock;

use crate::{Aggregate, Call, Db, Entry, Messages, Run, Trigger, erase};
pub(crate) use registry::Registry;

use wake::Wake;

const DEFAULT_MAX_ATTEMPTS: i64 = 10;
/// Retries and schedules come due without a notification.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Renewed while its job runs; a worker that dies frees its messages after it.
const LEASE: Duration = Duration::from_mins(5);
/// Below the lease, so a job whose renewals fail ends before another worker may claim it.
const JOB_TIMEOUT: Duration = Duration::from_mins(4);
/// A message no worker here handles may be a newer version's, queued during a rolling deploy.
const UNHEARD_KEPT_FOR: Duration = Duration::from_hours(1);
/// Loco's default worker count, so both frameworks start a job queue the same way.
const DEFAULT_IN_FLIGHT: usize = 2;

/// Any number of workers can share a database; each message is handled by one.
pub struct Worker {
    db: Db,
    messages: Messages,
    registry: Registry,
    max_attempts: i64,
    in_flight: usize,
    #[cfg(feature = "local-time")]
    local_looks: schedules::LocalLooks,
    #[cfg(feature = "remote-contracts")]
    feeds: Feeds,
}

impl Worker {
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self {
            messages: Messages::new(db.clone()),
            registry: Registry::default(),
            db,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            in_flight: DEFAULT_IN_FLIGHT,
            #[cfg(feature = "local-time")]
            local_looks: schedules::LocalLooks::default(),
            #[cfg(feature = "remote-contracts")]
            feeds: Feeds::default(),
        }
    }

    /// Runs `job` for each `E` raised.
    #[must_use]
    pub fn on<E: crate::Event, Args: 'static>(
        self,
        name: &'static str,
        job: impl Call<JobContext, Args>,
    ) -> Self {
        self.register(name, Trigger::On(E::NAME), erase(job))
    }

    /// At an interval, first as soon as the schedule is new; or at a `WallClock` time, first
    /// when the zone's clocks next show it.
    #[must_use]
    pub fn every<Args: 'static, J: Call<JobContext, Args>>(
        self,
        name: &'static str,
        cadence: impl Into<Cadence>,
        job: J,
    ) -> Self {
        self.register(name, Trigger::Every(cadence.into()), erase(job))
    }

    /// Each record's job runs with its own retries.
    #[must_use]
    pub fn each<A: Aggregate, QueryArgs: 'static, Args: 'static>(
        self,
        name: &'static str,
        interval: Duration,
        items: impl Call<JobContext, QueryArgs, Outcome = Vec<A>>,
        action: &'static str,
        job: impl Call<JobContext, Args>,
    ) -> Self {
        self.register(action, Trigger::Queued, erase(job)).register(
            name,
            Trigger::Every(Cadence::Interval(interval)),
            each::each(items, action),
        )
    }

    fn register(mut self, name: &'static str, trigger: Trigger, run: Run<JobContext>) -> Self {
        self.registry.add(Entry { name, trigger, run });
        self
    }

    /// A new mirror first copies every record its source has, once.
    #[cfg(feature = "contracts")]
    #[must_use]
    pub fn mirror<M: crate::Mirror>(self, name: &'static str) -> Self {
        const ONCE: Duration = Duration::from_hours(100 * 365 * 24);
        let backfill: &'static str = Box::leak(format!("{name}.backfill").into_boxed_str());
        let copy = move |source: crate::Dep<dyn crate::Lookup<M::Source>>, page| {
            crate::backfill_mirror::<M>(source, page, backfill)
        };
        let keep = |row: crate::MirrorRow<M>| async move { Ok(row) };
        let mut worker = self.every(backfill, ONCE, copy);
        worker.registry.add_taking_pruned(Entry {
            name,
            trigger: Trigger::On(<M::Changed as crate::Event>::NAME),
            run: erase(keep),
        });
        if worker
            .registry
            .job(crate::contracts::SWEEP_FORGOTTEN)
            .is_some()
        {
            return worker;
        }
        worker.every(
            crate::contracts::SWEEP_FORGOTTEN,
            Duration::from_hours(1),
            crate::contracts::sweep_forgotten,
        )
    }

    /// At least one, as none would leave every message unhandled:
    ///
    /// ```compile_fail,E0308
    /// fn never(worker: sofie_db::worker::Worker) -> sofie_db::worker::Worker {
    ///     worker.max_attempts(0)
    /// }
    /// ```
    #[must_use]
    pub fn max_attempts(mut self, max_attempts: NonZeroU16) -> Self {
        self.max_attempts = i64::from(max_attempts.get());
        self
    }

    #[must_use]
    pub fn batch(mut self, size: NonZeroUsize) -> Self {
        self.in_flight = size.get();
        self
    }

    /// Refuses jobs of one name and intervals out of range before any work starts.
    pub async fn run(self, shutdown: impl Future<Output = ()> + Send) -> Result<()> {
        self.registry.validate()?;
        publish_heard(&self);
        self.registry
            .schedules
            .store(&self.messages, self.db.clock().now().timestamp())
            .await?;
        #[cfg(feature = "metrics")]
        crate::metrics::watch_outbox(&self.db);
        #[cfg(feature = "remote-contracts")]
        let (shutdown, pulling) = self.feeds.start(&self.db, shutdown);
        let wake = &mut Wake::new(self.messages.clone());
        let worked = flight::work(&self, Some((pin!(shutdown), wake))).await;
        #[cfg(feature = "remote-contracts")]
        pulling.finish().await?;
        worked.map(drop)
    }

    /// Pulls every feed first; a failed job is not counted, it is retried once its backoff is over.
    pub async fn drain(&self) -> Result<usize> {
        self.drain_reporting()
            .await
            .map(|drained| drained.completed)
    }

    pub async fn drain_reporting(&self) -> Result<Drained> {
        self.registry.validate()?;
        publish_heard(self);
        self.registry
            .schedules
            .store(&self.messages, self.db.clock().now().timestamp())
            .await?;
        #[cfg(feature = "remote-contracts")]
        self.feeds.pull_all(&self.db).await?;
        flight::work(self, None).await
    }
}

#[derive(Debug, Default)]
pub struct Drained {
    pub completed: usize,
    pub failures: Vec<Failure>,
}

/// A panic counts as a failure too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub job: String,
    pub error: String,
}
