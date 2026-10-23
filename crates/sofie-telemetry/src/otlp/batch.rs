//! A bounded queue per signal, emptied by a task that exports it in batches.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use bytes::Bytes;
use sofie_core::unpoisoned;
use tokio::sync::{Notify, oneshot};
use tokio::time::Instant;

use super::settings::Settings;
use super::transport::Client;
use crate::outage::{self, Output};

pub(crate) trait Signal: Send + 'static {
    type Item: Send + 'static;
    const PATH: &'static str;
    const OUTPUT: Output;
    const CAPACITY: usize;
    const BATCH_SIZE: usize;
    const INTERVAL: Duration;

    fn encode(&self, items: Vec<Self::Item>, resource: &[u8]) -> Vec<u8>;

    fn report_dropped(_: &Queue<Self::Item>) {}
}

pub(crate) struct Queue<T> {
    state: Mutex<State<T>>,
    wake: Notify,
    capacity: usize,
    batch_size: usize,
    dropped: AtomicU64,
}

struct State<T> {
    items: VecDeque<T>,
    close: Option<oneshot::Sender<()>>,
    stopped: bool,
}

impl<T> Queue<T> {
    pub(crate) fn new(capacity: usize, batch_size: usize) -> Self {
        Self {
            state: Mutex::new(State {
                items: VecDeque::new(),
                close: None,
                stopped: false,
            }),
            wake: Notify::new(),
            capacity,
            batch_size,
            dropped: AtomicU64::new(0),
        }
    }

    /// Never waits: a full queue drops its oldest item and counts it.
    pub(crate) fn push(&self, item: T) {
        let mut state = self.state();
        let oldest = if state.items.len() >= self.capacity {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            state.items.pop_front()
        } else {
            None
        };
        state.items.push_back(item);
        let filled = state.items.len() == self.batch_size;
        drop(state);
        drop(oldest);
        if filled {
            self.wake.notify_one();
        }
    }

    pub(crate) fn take_dropped(&self) -> u64 {
        self.dropped.swap(0, Ordering::Relaxed)
    }

    pub(crate) fn close(&self) -> Closing {
        let mut state = self.state();
        if state.stopped {
            return Closing(None);
        }
        let (done, exported) = oneshot::channel();
        state.close = Some(done);
        drop(state);
        self.wake.notify_one();
        Closing(Some(exported))
    }

    fn state(&self) -> MutexGuard<'_, State<T>> {
        unpoisoned::lock(&self.state)
    }

    #[cfg(test)]
    pub(crate) fn queued(&self) -> Vec<T>
    where
        T: Clone,
    {
        self.state().items.iter().cloned().collect()
    }
}

/// Awaited apart from its request, so every signal's last export runs under one deadline.
pub(crate) struct Closing(Option<oneshot::Receiver<()>>);

impl Closing {
    pub(crate) async fn finished_by(self, deadline: Instant) -> bool {
        let Some(exported) = self.0 else {
            return true;
        };
        matches!(
            tokio::time::timeout_at(deadline, exported).await,
            Ok(Ok(()))
        )
    }
}

/// Must be called inside a Tokio runtime.
pub(crate) fn start<S: Signal>(signal: S, settings: Arc<Settings>) -> Arc<Queue<S::Item>> {
    let queue = Arc::new(Queue::new(S::CAPACITY, S::BATCH_SIZE));
    let client = Client::new(Arc::clone(&settings), S::PATH);
    let running = Running(Arc::clone(&queue));
    tokio::spawn(run(signal, client, settings, running));
    queue
}

/// Marks the queue stopped however the task ends, so a close never waits on a task that is gone.
struct Running<T>(Arc<Queue<T>>);

impl<T> Drop for Running<T> {
    fn drop(&mut self) {
        let mut state = self.0.state();
        state.stopped = true;
        state.close = None;
    }
}

struct Round<T> {
    items: VecDeque<T>,
    close: Option<oneshot::Sender<()>>,
}

async fn run<S: Signal>(
    signal: S,
    mut client: Client,
    settings: Arc<Settings>,
    running: Running<S::Item>,
) {
    let queue = &running.0;
    loop {
        if !round_ready(queue) {
            // Woken early by a full batch or a close; otherwise the interval ends the wait.
            let _ = tokio::time::timeout(S::INTERVAL, queue.wake.notified()).await;
        }
        let Round { mut items, close } = take_round(queue);
        while !items.is_empty() {
            let batch: Vec<_> = items.drain(..items.len().min(S::BATCH_SIZE)).collect();
            let body = Bytes::from(signal.encode(batch, &settings.resource));
            let result = client.export(body).await;
            let newly_failing = outage::report(S::OUTPUT, result.is_ok());
            if let Err(error) = result
                && newly_failing
            {
                tracing::warn!(telemetry.output = S::OUTPUT.name(), error = %error, "OTLP export failing");
            }
        }
        S::report_dropped(queue);
        if let Some(done) = close {
            // The closing caller may have stopped waiting.
            let _ = done.send(());
            return;
        }
    }
}

fn round_ready<T>(queue: &Queue<T>) -> bool {
    let state = queue.state();
    state.items.len() >= queue.batch_size || state.close.is_some()
}

fn take_round<T>(queue: &Queue<T>) -> Round<T> {
    let mut state = queue.state();
    Round {
        items: std::mem::take(&mut state.items),
        close: state.close.take(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::time::Instant;

    use super::Queue;

    #[tokio::test]
    async fn stalled_closes_requested_together_end_within_one_wait() {
        let stalled: [Queue<u8>; 3] = [Queue::new(1, 1), Queue::new(1, 1), Queue::new(1, 1)];
        let wait = Duration::from_millis(200);
        let started = Instant::now();
        let closings = stalled.each_ref().map(Queue::close);

        for closing in closings {
            assert!(!closing.finished_by(started + wait).await);
        }

        assert!(started.elapsed() < wait * 2, "{:?}", started.elapsed());
    }

    #[test]
    fn a_full_queue_drops_its_oldest_item_and_counts_it() {
        let queue = Queue::new(2, 2);

        queue.push(1);
        queue.push(2);
        queue.push(3);

        assert_eq!(queue.queued(), [2, 3]);
        assert_eq!(queue.take_dropped(), 1);
        assert_eq!(queue.take_dropped(), 0);
    }
}
