//! Callers only send into a channel, which costs no lock; the metrics thread does the rest.

mod aggregate;
#[cfg(feature = "otlp")]
mod export;

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use crate::InitError;
use aggregate::Aggregates;

const MAX_LABELS: usize = 4;
const RENDER_WAIT: Duration = Duration::from_secs(2);
/// A level that costs a query is read this stale at most.
const POLL_INTERVAL: Duration = Duration::from_secs(15);
/// Polled, not `recv`: a parked receiver costs every send a waker signal, a syscall each.
const DRAIN_INTERVAL: Duration = Duration::from_millis(50);

/// Past it a measurement is dropped and counted; the std channel cannot drop its oldest, and
/// either loses one sample of a sum alike.
const QUEUED: usize = 65_536;

static RECORDER: OnceLock<SyncSender<Message>> = OnceLock::new();
static DROPPED: AtomicU64 = AtomicU64::new(0);
static DROPPED_MEASUREMENTS: Instrument = Instrument {
    name: "sofie.metrics.dropped",
    description: "Measurements dropped because the metrics thread fell behind.",
    kind: Kind::Counter,
    labels: &[],
};

#[derive(Debug)]
pub struct Instrument {
    pub name: &'static str,
    pub description: &'static str,
    pub kind: Kind,
    pub labels: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Counter,
    Duration,
    Gauge,
}

/// Only `'static` names or codes, so text made while serving (ids, paths) cannot become a label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Label {
    Name(&'static str),
    Code(u16),
}

impl From<&'static str> for Label {
    fn from(name: &'static str) -> Self {
        Self::Name(name)
    }
}

impl From<u16> for Label {
    fn from(code: u16) -> Self {
        Self::Code(code)
    }
}

type Labels = [Label; MAX_LABELS];

pub(crate) struct Measurement {
    instrument: &'static Instrument,
    labels: Labels,
    value: f64,
}

enum Message {
    Measured(Measurement),
    Gauge(&'static Instrument, fn() -> f64),
    Poll(Box<dyn Fn() + Send>),
    Render(tokio::sync::oneshot::Sender<String>),
    /// A snapshot for the OTLP exporter now, answered once it is queued.
    #[cfg(feature = "otlp")]
    Export(tokio::sync::oneshot::Sender<()>),
}

/// Until it starts, every measurement is dropped at the cost of one load.
pub fn start() -> Result<(), InitError> {
    if RECORDER.get().is_some() {
        return Ok(());
    }
    let (sender, receiver) = mpsc::sync_channel(QUEUED);
    if RECORDER.set(sender).is_err() {
        return Ok(());
    }
    // Should this fail, the receiver is dropped and measurements with it.
    let aggregates = Aggregates::new()?;
    thread::Builder::new()
        .name("sofie-metrics".to_owned())
        .spawn(move || run(&receiver, aggregates))
        .map(drop)
        .map_err(|error| InitError::new(format!("starting the metrics thread: {error}")))
}

fn run(receiver: &Receiver<Message>, mut aggregates: Aggregates) {
    let mut next_poll = Instant::now() + POLL_INTERVAL;
    while drain(receiver, &mut aggregates) {
        if Instant::now() >= next_poll {
            aggregates.poll();
            next_poll = Instant::now() + POLL_INTERVAL;
        }
        #[cfg(feature = "otlp")]
        aggregates.export_when_due();
        thread::sleep(DRAIN_INTERVAL);
    }
}

fn drain(receiver: &Receiver<Message>, aggregates: &mut Aggregates) -> bool {
    let dropped = DROPPED.swap(0, Ordering::Relaxed);
    if dropped > 0 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "Prometheus samples are floats; counts this large are not reached"
        )]
        aggregates.receive(Message::Measured(Measurement {
            instrument: &DROPPED_MEASUREMENTS,
            labels: [Label::Name(""); MAX_LABELS],
            value: dropped as f64,
        }));
    }
    loop {
        match receiver.try_recv() {
            Ok(message) => aggregates.receive(message),
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
}

pub fn record(instrument: &'static Instrument, labels: &[Label], value: f64) {
    let Some(recorder) = RECORDER.get() else {
        return;
    };
    debug_assert_eq!(
        labels.len(),
        instrument.labels.len(),
        "{} takes the labels {:?}",
        instrument.name,
        instrument.labels
    );
    debug_assert!(value.is_finite(), "{} recorded {value}", instrument.name);
    let mut placed = [Label::Name(""); MAX_LABELS];
    for (place, label) in placed.iter_mut().zip(labels) {
        *place = *label;
    }
    let measured = Message::Measured(Measurement {
        instrument,
        labels: placed,
        value,
    });
    offer(recorder, measured, &DROPPED);
}

/// A stopped metrics thread only means the measurement is not kept.
fn offer(recorder: &SyncSender<Message>, message: Message, dropped: &AtomicU64) {
    if let Err(TrySendError::Full(_)) = recorder.try_send(message) {
        dropped.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn record_duration(instrument: &'static Instrument, labels: &[Label], elapsed: Duration) {
    record(instrument, labels, elapsed.as_secs_f64());
}

pub fn gauge(instrument: &'static Instrument, read: fn() -> f64) {
    if let Some(recorder) = RECORDER.get() {
        let _ = recorder.send(Message::Gauge(instrument, read));
    }
}

/// `poll` must start its query elsewhere, so the thread never waits on it.
pub fn poll(poll: impl Fn() + Send + 'static) {
    if let Some(recorder) = RECORDER.get() {
        let _ = recorder.send(Message::Poll(Box::new(poll)));
    }
}

pub async fn prometheus() -> Option<String> {
    let recorder = RECORDER.get()?;
    let (answer, rendered) = tokio::sync::oneshot::channel();
    recorder.try_send(Message::Render(answer)).ok()?;
    tokio::time::timeout(RENDER_WAIT, rendered).await.ok()?.ok()
}

#[cfg(feature = "otlp")]
pub(crate) use export::close;

#[cfg(test)]
mod tests {
    use super::*;

    static TESTED: Instrument = Instrument {
        name: "test.offered",
        description: "Offered.",
        kind: Kind::Counter,
        labels: &[],
    };

    fn measured() -> Message {
        Message::Measured(Measurement {
            instrument: &TESTED,
            labels: [Label::Name(""); MAX_LABELS],
            value: 1.0,
        })
    }

    #[test]
    fn a_measurement_past_a_full_queue_is_dropped_and_counted() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let dropped = AtomicU64::new(0);

        offer(&sender, measured(), &dropped);
        offer(&sender, measured(), &dropped);

        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert!(receiver.try_recv().is_ok());
        assert!(receiver.try_recv().is_err());
    }
}
