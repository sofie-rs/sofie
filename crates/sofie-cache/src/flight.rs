use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sofie_core::unpoisoned;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

#[derive(Default)]
pub(crate) struct Flights {
    running: Mutex<HashMap<String, Boarded>>,
}

struct Boarded {
    lock: Arc<AsyncMutex<Option<Landed>>>,
    passengers: usize,
}

pub(crate) struct Landed {
    pub(crate) epoch: u64,
    pub(crate) value: Arc<str>,
    pub(crate) recomputed: bool,
}

pub(crate) struct Flight<'f> {
    flights: &'f Flights,
    key: String,
    lock: Option<OwnedMutexGuard<Option<Landed>>>,
}

impl Flights {
    /// Counted before it waits, so a caller cancelled while waiting still leaves.
    pub(crate) async fn board(&self, key: &str) -> Flight<'_> {
        let lock = {
            let mut running = self.running();
            let boarded = running.entry(key.to_owned()).or_insert_with(|| Boarded {
                lock: Arc::default(),
                passengers: 0,
            });
            boarded.passengers += 1;
            Arc::clone(&boarded.lock)
        };
        let mut flight = Flight {
            flights: self,
            key: key.to_owned(),
            lock: None,
        };
        flight.lock = Some(lock.lock_owned().await);
        flight
    }

    fn running(&self) -> std::sync::MutexGuard<'_, HashMap<String, Boarded>> {
        unpoisoned::lock(&self.running)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.running().len()
    }
}

impl Flight<'_> {
    pub(crate) fn landed(&self) -> Option<&Landed> {
        self.lock.as_deref()?.as_ref()
    }

    pub(crate) fn land(&mut self, landed: Landed) {
        if let Some(lock) = &mut self.lock {
            **lock = Some(landed);
        }
    }
}

impl Drop for Flight<'_> {
    fn drop(&mut self) {
        drop(self.lock.take());
        let mut running = self.flights.running();
        let Some(boarded) = running.get_mut(&self.key) else {
            return;
        };
        boarded.passengers -= 1;
        if boarded.passengers == 0 {
            running.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn one_key_flies_once_at_a_time_and_leaves_nothing_behind() {
        let flights = Arc::new(Flights::default());
        let inside = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let (flights, inside, most) =
                    (Arc::clone(&flights), Arc::clone(&inside), Arc::clone(&most));
                tokio::spawn(async move {
                    let _flight = flights.board("key").await;
                    let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                    most.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    inside.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap();
        }

        assert_eq!(most.load(Ordering::SeqCst), 1);
        assert_eq!(flights.len(), 0);
    }
}
