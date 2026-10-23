use std::sync::Arc;
use std::time::Duration;

use sofie_core::{Error, Result};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::{Registry, Worker};
use crate::{Db, Entry, FeedSource, Trigger};

const PRUNE: &str = "sofie.feed.prune";

#[derive(Default)]
pub struct Feeds {
    published: Vec<&'static str>,
    pulled: Vec<Arc<dyn FeedSource>>,
}

impl Feeds {
    /// Old entries are pruned hourly.
    #[must_use]
    pub fn publish(mut self, event: &'static str) -> Self {
        self.published.push(event);
        self
    }

    /// Each pulled event is delivered once.
    #[must_use]
    pub fn pull(mut self, sources: impl IntoIterator<Item = Arc<dyn FeedSource>>) -> Self {
        self.pulled.extend(sources);
        self
    }

    /// Kept off `Worker`'s own methods, which only run what is registered.
    #[must_use]
    pub fn attach(self, mut worker: Worker) -> Worker {
        self.register(&mut worker.registry);
        worker.feeds = self;
        worker
    }

    fn register(&self, registry: &mut Registry) {
        for event in &self.published {
            // Named after its event: one feed job per event, and no event is also a listener's name.
            registry.add_taking_pruned(Entry {
                name: event,
                trigger: Trigger::On(event),
                run: crate::feed::append(event),
            });
        }
        if !self.published.is_empty() && registry.job(PRUNE).is_none() {
            registry.add(Entry {
                name: PRUNE,
                trigger: Trigger::Every(Duration::from_hours(1).into()),
                run: crate::feed::prune(),
            });
        }
    }

    pub(super) async fn pull_all(&self, db: &Db) -> Result<()> {
        for source in &self.pulled {
            while crate::feed::pull_once(db, source.as_ref()).await? {}
        }
        Ok(())
    }

    /// Each pull finishes the page it is on once `shutdown` completes.
    pub(super) fn start(
        &self,
        db: &Db,
        shutdown: impl Future<Output = ()> + Send,
    ) -> (impl Future<Output = ()> + Send, Pulling) {
        let (stop, stopped) = watch::channel(false);
        let pulls = self
            .pulled
            .iter()
            .map(|source| {
                let pull = crate::feed::pull(db.clone(), Arc::clone(source), stopped.clone());
                tokio::spawn(pull)
            })
            .collect();
        let shutdown = async move {
            shutdown.await;
            let _ = stop.send(true);
        };
        (shutdown, Pulling(pulls))
    }
}

pub(super) struct Pulling(Vec<JoinHandle<()>>);

impl Pulling {
    pub(super) async fn finish(self) -> Result<()> {
        for pull in self.0 {
            pull.await
                .map_err(|error| Error::internal("pulling another app's events", error))?;
        }
        Ok(())
    }
}
