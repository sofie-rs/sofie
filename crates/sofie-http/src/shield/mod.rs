//! Attacks told apart by behaviour, never payload: requests only send signals to a detector thread.

mod arguments;
mod blocks;
mod detector;
mod effects;
mod report;
mod rule;
mod window;

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::time::Duration;

use http::StatusCode;
use sofie_core::{Clock, Error, Result, Uuid};
use sofie_db::Db;

pub use arguments::{ActiveBlock, ActiveBlocks, ShieldBlocks, SignIns};
pub(crate) use detector::Safety;
pub use rule::Thresholds;

use self::blocks::{Blocks, Grade, Key};
use self::detector::{Detector, Message, Signal};
use self::effects::Effects;
use self::rule::Rule;
use crate::answer::Response;
use crate::pipeline::Asked;
use crate::throttle::{Limiter, Rate, Requester};
use crate::trace::RouteLabel;

/// Past it a signal is dropped and counted rather than making a request wait.
const QUEUE: usize = 16_384;
const SLOW_RATE: Rate = Rate::per_second(1);
/// Asked for by browsers and crawlers on their own, so missing ones are no scan.
const WELL_KNOWN: [&str; 6] = [
    "/favicon.ico",
    "/robots.txt",
    "/sitemap.xml",
    "/.well-known/security.txt",
    "/.well-known/change-password",
    "/.well-known/traffic-advice",
];

/// `SOFIE_SHIELD`: `on`, `dry-run`, `off`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShieldMode {
    Enforce,
    DryRun,
    /// For tests, `sofie bench` and `sofie fuzz`, whose traffic looks like an attack.
    Off,
}

impl ShieldMode {
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim() {
            "on" => Ok(Self::Enforce),
            "dry-run" => Ok(Self::DryRun),
            "off" => Ok(Self::Off),
            other => Err(Error::internal_message(format!(
                "shield mode `{other}` is not `on`, `dry-run` or `off`"
            ))),
        }
    }

    /// A development build only reports, so a developer's mistakes lock nobody out.
    fn of_build() -> Self {
        if cfg!(debug_assertions) {
            Self::DryRun
        } else {
            Self::Enforce
        }
    }
}

/// `sofie::run` reads it from `SOFIE_SHIELD`, `SOFIE_SHIELD_THRESHOLDS` and `SOFIE_SHIELD_SHARED`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shield {
    mode: ShieldMode,
    thresholds: Thresholds,
    shared: bool,
}

impl Default for Shield {
    fn default() -> Self {
        Self::new(ShieldMode::of_build())
    }
}

impl Shield {
    #[must_use]
    pub fn new(mode: ShieldMode) -> Self {
        Self {
            mode,
            thresholds: Thresholds::default(),
            shared: false,
        }
    }

    #[must_use]
    pub fn thresholds(self, thresholds: Thresholds) -> Self {
        Self { thresholds, ..self }
    }

    #[must_use]
    pub fn shared(self) -> Self {
        Self {
            shared: true,
            ..self
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct Guard(Option<Arc<Sentinel>>);

pub(crate) struct Sentinel {
    blocks: Arc<Blocks>,
    sender: SyncSender<Message>,
    dropped: Arc<AtomicU64>,
    clock: Clock,
    slowed: Limiter,
    shared: Option<Db>,
}

/// No signal: the shield's own refusal, or an answer to a call another app proved it made.
#[derive(Clone, Copy)]
struct Shielded;

#[derive(Clone, Copy)]
struct SignedIn(Uuid);

impl Guard {
    pub(crate) fn start(
        app: &mut crate::App,
        background: &mut crate::background::Background,
    ) -> Result<Self> {
        let shield = std::mem::take(&mut app.serving.shield);
        if shield.mode == ShieldMode::Off {
            return Ok(Self(None));
        }
        let areas: Vec<&str> = app.routes.iter().map(|route| route.audience.name).collect();
        shield.thresholds.check_areas(&areas)?;
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|error| Error::internal("starting the shield outside a runtime", error))?;
        let db = app.db.clone();
        let effects = Effects {
            runtime: runtime.clone(),
            db: db.clone(),
            members: app.members.clone(),
            shared: shield.shared,
        };
        let dry_run = shield.mode == ShieldMode::DryRun;
        let safety = Safety {
            #[cfg(feature = "trusted-proxies")]
            proxies: app.serving.proxies.clone(),
        };
        let detector = Detector::new(shield.thresholds, safety, dry_run, Some(effects));
        let (blocks, dropped) = (Arc::clone(&detector.blocks), Arc::clone(&detector.dropped));
        if shield.shared {
            background.spawn(effects::follow(
                db.clone(),
                db.clock(),
                Arc::downgrade(&blocks),
            ));
        }
        let (sender, receiver) = mpsc::sync_channel(QUEUE);
        detector::start(receiver, detector, db.clock())?;
        Ok(Self(Some(Arc::new(Sentinel {
            blocks,
            sender,
            dropped,
            clock: db.clock(),
            slowed: Limiter::new(SLOW_RATE),
            shared: shield.shared.then_some(db),
        }))))
    }

    /// By principal when signed in, so an account is not held to what its neighbours did, but
    /// for the sign-ins and sign-ups counted by address: any session cookie would lift those.
    pub(crate) fn refusal(&self, asked: Asked<'_>, principal: Option<Uuid>) -> Option<Response> {
        let sentinel = self.0.as_ref()?;
        if sentinel.blocks.is_empty() {
            return None;
        }
        let now = sentinel.clock.now().timestamp();
        let requester = Requester::of(principal, asked.client);
        let address = Requester::of(None, asked.client);
        let (requester, block) = sentinel
            .blocks
            .find(Key::Caller(requester), now.as_microsecond())
            .map(|block| (requester, block))
            .or_else(|| {
                let block = sentinel
                    .blocks
                    .find(Key::Caller(address), now.as_microsecond())?;
                matches!(block.rule, Rule::SignIn | Rule::SignUp).then_some((address, block))
            })?;
        let wait = match block.grade {
            Grade::Slow => sentinel.slowed.admit(requester, now).err()?,
            Grade::Block => Duration::from_micros(
                u64::try_from(block.until - now.as_microsecond()).unwrap_or_default(),
            ),
            Grade::Observe | Grade::Lock => return None,
        };
        let mut response = crate::answer::failure(Error::TooManyRequests(wait), asked.caller);
        response.extensions_mut().insert(Shielded);
        Some(response)
    }

    pub(crate) fn observe(
        &self,
        asked: Asked<'_>,
        route: Option<RouteLabel<'_>>,
        response: &Response,
    ) {
        let Some(sentinel) = &self.0 else {
            return;
        };
        let area = route.and_then(|route| route.area);
        let Some(rule) = rule_of(response.status(), area.is_some()) else {
            return;
        };
        if rule == Rule::Scan && is_well_known(asked.request.uri.path()) {
            return;
        }
        if response.extensions().get::<Shielded>().is_some() {
            return;
        }
        let principal = response
            .extensions()
            .get::<SignedIn>()
            .map(|signed| signed.0);
        let key = Key::Caller(Requester::of(principal, asked.client));
        sentinel.send(rule, key, asked.client, area);
    }

    pub(crate) fn connection_failed(&self, peer: IpAddr) {
        if let Some(sentinel) = &self.0 {
            let key = Key::Caller(Requester::of(None, peer));
            sentinel.send(Rule::Protocol, key, peer, None);
        }
    }

    pub(crate) fn for_request(
        &self,
        asked: Asked<'_>,
        area: &'static str,
    ) -> Option<RequestShield> {
        Some(RequestShield {
            sentinel: Arc::clone(self.0.as_ref()?),
            address: asked.client,
            area,
        })
    }
}

impl Sentinel {
    fn send(&self, rule: Rule, key: Key, address: IpAddr, area: Option<&'static str>) {
        let signal = Signal {
            rule,
            key,
            address,
            area,
            at: self.now(),
        };
        self.post(Message::Signal(signal));
    }

    fn post(&self, message: Message) {
        if self.sender.try_send(message).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn now(&self) -> i64 {
        self.clock.now().timestamp().as_microsecond()
    }
}

#[derive(Clone)]
pub(crate) struct RequestShield {
    sentinel: Arc<Sentinel>,
    address: IpAddr,
    area: &'static str,
}

/// A contract call's 404 or 400 is the calling app's bug, not its address probing this one.
#[cfg(feature = "remote-contracts")]
pub(crate) fn unobserved(mut response: Response) -> Response {
    response.extensions_mut().insert(Shielded);
    response
}

/// Read back by [`Guard::observe`] once the response leaves the session.
pub(crate) fn tag(mut response: Response, principal: Option<Uuid>) -> Response {
    if let Some(principal) = principal
        && rule_of(response.status(), true).is_some()
    {
        response.extensions_mut().insert(SignedIn(principal));
    }
    response
}

/// Safari also asks for sized icons, such as `/apple-touch-icon-120x120-precomposed.png`.
fn is_well_known(path: &str) -> bool {
    WELL_KNOWN.contains(&path)
        || path
            .strip_prefix("/apple-touch-icon")
            .and_then(|rest| rest.strip_suffix(".png"))
            .is_some_and(|rest| !rest.contains('/'))
}

/// On a matched route a missing record is a probe, anywhere else a scan.
fn rule_of(status: StatusCode, matched: bool) -> Option<Rule> {
    match status {
        StatusCode::NOT_FOUND if matched => Some(Rule::Probe),
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED => Some(Rule::Scan),
        StatusCode::FORBIDDEN => Some(Rule::Refused),
        StatusCode::BAD_REQUEST | StatusCode::PAYLOAD_TOO_LARGE => Some(Rule::Malformed),
        StatusCode::TOO_MANY_REQUESTS => Some(Rule::Throttled),
        _ => None,
    }
}

impl crate::Serving {
    #[must_use]
    pub fn shield(mut self, shield: Shield) -> Self {
        self.shield = shield;
        self
    }
}

#[cfg(test)]
mod tests;
