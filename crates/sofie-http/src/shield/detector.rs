use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread;
use std::time::Duration;

use sofie_core::Clock;

use super::blocks::{Block, Blocks, Grade, Key};
use super::effects::Effects;
use super::report;
use super::rule::{Rule, Thresholds};
use super::window::Window;
use crate::throttle::Requester;

/// Polled, not waited on: a receiver parked in `recv` makes every send a wake-up syscall.
const DRAIN_INTERVAL: Duration = Duration::from_millis(50);
const SWEEP_EVERY: u32 = 20;
const PERIOD: Duration = Duration::from_mins(15);
/// Past it, after forgetting idle ones, a signal for a new key is dropped and counted.
const MAX_WINDOWS: usize = 131_072;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Signal {
    pub(crate) rule: Rule,
    pub(crate) key: Key,
    /// For the safety check only.
    pub(crate) address: IpAddr,
    pub(crate) area: Option<&'static str>,
    pub(crate) at: i64,
}

#[derive(Clone, Copy)]
pub(crate) enum Message {
    Signal(Signal),
    /// Its counts start over.
    Lifted(Key),
}

/// Callers it cannot tell apart from others, such as a proxy in front of the app.
#[derive(Clone, Default)]
pub(crate) struct Safety {
    #[cfg(feature = "trusted-proxies")]
    pub(crate) proxies: crate::TrustedProxies,
}

impl Safety {
    /// Loopback is a local proxy or the operator, not one client; a trusted proxy speaks for many.
    #[cfg_attr(
        not(feature = "trusted-proxies"),
        expect(clippy::unused_self, reason = "only trusted proxies are kept")
    )]
    fn may_block(&self, address: IpAddr) -> bool {
        let address = address.to_canonical();
        if address.is_unspecified() || address.is_loopback() {
            return false;
        }
        #[cfg(feature = "trusted-proxies")]
        if self.proxies.is_trusted(address) {
            return false;
        }
        true
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Decision {
    pub(crate) key: Key,
    pub(crate) rule: Rule,
    pub(crate) grade: Grade,
    pub(crate) count: u32,
    pub(crate) threshold: u32,
    pub(crate) area: Option<&'static str>,
    pub(crate) at: i64,
    pub(crate) until: i64,
}

pub(crate) struct Detector {
    pub(crate) thresholds: Thresholds,
    pub(crate) safety: Safety,
    pub(crate) dry_run: bool,
    pub(crate) blocks: Arc<Blocks>,
    pub(crate) effects: Option<Effects>,
    windows: HashMap<(Key, Rule), Window>,
    pub(crate) dropped: Arc<AtomicU64>,
}

impl Detector {
    pub(crate) fn new(
        thresholds: Thresholds,
        safety: Safety,
        dry_run: bool,
        effects: Option<Effects>,
    ) -> Self {
        Self {
            thresholds,
            safety,
            dry_run,
            blocks: Arc::default(),
            effects,
            windows: HashMap::new(),
            dropped: Arc::default(),
        }
    }

    pub(crate) fn handle(&mut self, message: Message) {
        match message {
            Message::Signal(signal) => {
                if let Some(decision) = self.decide(signal) {
                    self.apply(decision);
                }
            }
            Message::Lifted(key) => self.windows.retain(|(held, _), _| *held != key),
        }
    }

    fn decide(&mut self, signal: Signal) -> Option<Decision> {
        if let Key::Caller(Requester::Address(_)) = signal.key
            && !self.safety.may_block(signal.address)
        {
            return None;
        }
        let limit = self.thresholds.limit(signal.area, signal.rule);
        let length = micros(limit.window);
        let window = self.window(signal, length)?;
        let count = window.add(signal.at);
        let grade = grade_of(signal.rule, count, limit.count)?;
        let renewed = match window.decided {
            Some((decided, until)) => grade > decided || signal.at >= until,
            None => true,
        };
        if !renewed {
            return None;
        }
        let holds = if grade == Grade::Observe {
            length
        } else {
            micros(PERIOD)
        };
        let until = signal.at.saturating_add(holds);
        window.decided = Some((grade, until));
        Some(Decision {
            key: signal.key,
            rule: signal.rule,
            grade,
            count,
            threshold: limit.count,
            area: signal.area,
            at: signal.at,
            until,
        })
    }

    fn window(&mut self, signal: Signal, length: i64) -> Option<&mut Window> {
        let entry = (signal.key, signal.rule);
        if self.windows.len() >= MAX_WINDOWS && !self.windows.contains_key(&entry) {
            self.windows.retain(|_, window| !window.is_idle(signal.at));
            if self.windows.len() >= MAX_WINDOWS {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        }
        Some(
            self.windows
                .entry(entry)
                .or_insert_with(|| Window::new(length, signal.at)),
        )
    }

    fn apply(&self, decision: Decision) {
        report::decided(&decision, self.dry_run);
        if self.dry_run || decision.grade == Grade::Observe {
            return;
        }
        let block = Block {
            rule: decision.rule,
            grade: decision.grade,
            until: decision.until,
            shared: false,
        };
        if !self.blocks.insert(decision.key, block, decision.at) {
            report::table_full(&decision);
            return;
        }
        if let Some(effects) = &self.effects {
            effects.decided(&decision);
        }
    }

    fn sweep(&mut self, now: i64) {
        self.blocks.sweep(now);
        self.windows.retain(|_, window| !window.is_idle(now));
    }
}

/// Slowed at the threshold, blocked at twice it, reported from half.
fn grade_of(rule: Rule, count: u32, threshold: u32) -> Option<Grade> {
    let harsh = if matches!(rule, Rule::Account | Rule::AccountTotal) {
        Grade::Lock
    } else if count >= threshold.saturating_mul(2) {
        Grade::Block
    } else {
        Grade::Slow
    };
    if count >= threshold {
        return Some(harsh);
    }
    (count.saturating_mul(2) >= threshold).then_some(Grade::Observe)
}

pub(crate) fn micros(duration: Duration) -> i64 {
    i64::try_from(duration.as_micros()).unwrap_or(i64::MAX)
}

pub(crate) fn start(
    receiver: Receiver<Message>,
    detector: Detector,
    clock: Clock,
) -> sofie_core::Result<()> {
    thread::Builder::new()
        .name("sofie-shield".to_owned())
        .spawn(move || run(&receiver, detector, &clock))
        .map(drop)
        .map_err(|error| sofie_core::Error::internal("starting the shield's thread", error))
}

fn run(receiver: &Receiver<Message>, mut detector: Detector, clock: &Clock) {
    let mut drains = 0_u32;
    loop {
        loop {
            match receiver.try_recv() {
                Ok(message) => detector.handle(message),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        report::dropped(detector.dropped.swap(0, Ordering::Relaxed));
        drains += 1;
        if drains.is_multiple_of(SWEEP_EVERY) {
            detector.sweep(clock.now().timestamp().as_microsecond());
        }
        thread::sleep(DRAIN_INTERVAL);
    }
}
