//! Open screens told when what they read changed; each refetches its fragments through its
//! own route, so every guard applies again and nothing rendered travels down the stream.

use std::collections::{BTreeSet, HashMap};
use std::convert::Infallible;
use std::future::poll_fn;
use std::net::SocketAddr;
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http::header::{CONTENT_TYPE, RETRY_AFTER};
use http::request::Parts;
use http::{HeaderValue, Method, StatusCode};
use http_body_util::{BodyExt, Either, Full};
use hyper::body::{Frame, Incoming};
use sofie_auth::{Signed, SigningKeys};
use sofie_core::{Error, Result, Uuid, unpoisoned};
use sofie_db::{Announced, Reach, Topic, Topics};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::time::{Interval, MissedTickBehavior, Sleep};

use crate::answer::{self, HTML, Response, TEXT};
use crate::caller::Caller;
use crate::pipeline::Pipeline;
use crate::routes::{Audience, Route, RouteRules, View};
use crate::target::Target;
use crate::throttle::Requester;
use crate::{Area, PageRoute};

const PATH: &str = "/_sofie/live/";
const MARKER: &str = "sofie-live";
/// Tabs of one person, or callers behind one address (IPv6 by /64).
const PER_HOLDER: usize = 8;
const PER_PROCESS: usize = 10_000;
/// More than this and a screen hears every change instead.
const TOPICS_LIMIT: usize = 32;
/// Proxies close a connection idle for a minute or so.
const HEARTBEAT: Duration = Duration::from_secs(20);
/// A stream reconnects this often, so a session that ended stops hearing.
const LIFETIME: Duration = Duration::from_mins(10);
/// A burst of commits is one refetch.
const SETTLE: Duration = Duration::from_millis(50);
/// A stream's seal holds about this long; a reconnect refused once it passed, or twice in a row,
/// refetches the fragments, whose marker carries a fresh seal.
const SEAL_LIFETIME: i64 = 60 * 60;
/// Seals end on these steps, so a page refetched within one names the same stream.
const SEAL_STEP: i64 = 10 * 60;
const RETRY: &str = "5";
const SEAL: &str = "seal=";
const UNTIL: &str = "until=";

const OPENED: &[u8] = b": open\n\n";
const BEAT: &[u8] = b": beat\n\n";
const CHANGED: &[u8] = b"data: changed\n\n";
/// Told apart from a lost stream, so the page reconnects without backoff.
const END: &[u8] = b"event: end\ndata: end\n\n";

pub(crate) type Body = Either<Full<Bytes>, Events>;

impl<A: Area> PageRoute<'_, A> {
    /// Refetches the screen's `fragment` whenever a record its loader read changes.
    pub fn live(&mut self, fragment: &str) -> &mut Self {
        let fragment = fragment.to_owned();
        self.rule(|rules| rules.live.push(fragment))
    }
}

#[derive(Default)]
struct Open {
    total: usize,
    held: HashMap<Requester, usize>,
}

pub(crate) struct Live {
    areas: HashMap<&'static str, Audience>,
    open: Arc<Mutex<Open>>,
    closing: watch::Sender<bool>,
}

impl Live {
    pub(crate) fn new(routes: &[Route]) -> Result<Self> {
        let mut areas = HashMap::new();
        for route in routes.iter().filter(|route| !route.rules.live.is_empty()) {
            require_screen(route)?;
            require_plain(route, route.audience.name)?;
            for fragment in &route.rules.live {
                require_plain(route, fragment)?;
            }
            areas.insert(route.audience.name, route.audience);
        }
        Ok(Self {
            areas,
            open: Arc::default(),
            closing: watch::channel(false).0,
        })
    }

    /// Ends every stream, so a stopping server is not held open by them.
    pub(crate) fn close(&self) {
        self.closing.send_replace(true);
    }

    fn admit(&self, holder: Requester) -> Option<Admitted> {
        let mut open = unpoisoned::lock(&self.open);
        let held = open.held.get(&holder).copied().unwrap_or(0);
        if open.total >= PER_PROCESS || held >= PER_HOLDER {
            return None;
        }
        open.total += 1;
        open.held.insert(holder, held + 1);
        Some(Admitted {
            open: Arc::clone(&self.open),
            holder,
        })
    }
}

fn require_screen(route: &Route) -> Result<()> {
    if matches!(route.view, View::Screen(_)) {
        return Ok(());
    }
    Err(route_error(route, "is live but renders no screen"))
}

/// Written into the page and its stream's path unescaped.
fn require_plain(route: &Route, name: &str) -> Result<()> {
    let plain = !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    if plain {
        return Ok(());
    }
    Err(route_error(
        route,
        &format!("is live by `{name}`, which is not only letters, digits, `-` and `_`"),
    ))
}

fn route_error(route: &Route, problem: &str) -> Error {
    Error::internal_message(format!("route {} {} {problem}", route.method, route.path))
}

/// Frees its place when its stream ends, however it ends.
struct Admitted {
    open: Arc<Mutex<Open>>,
    holder: Requester,
}

impl Drop for Admitted {
    fn drop(&mut self) {
        let mut open = unpoisoned::lock(&self.open);
        open.total -= 1;
        let held = open.held.get_mut(&self.holder).map_or(0, |held| {
            *held -= 1;
            *held
        });
        if held == 0 {
            open.held.remove(&self.holder);
        }
    }
}

pub(crate) async fn heard<F: Future>(rules: &RouteRules, load: F) -> (F::Output, Option<Topics>) {
    if rules.live.is_empty() {
        return (load.await, None);
    }
    let (output, topics) = sofie_db::track_topics(load).await;
    (output, Some(topics))
}

/// The page carries its stream and fragments; a fragment carries them out of band, so a
/// refetch that read other tables listens to those.
pub(crate) async fn marked(
    response: Response,
    heard: Option<Topics>,
    target: &Target,
    caller: Caller<'_>,
    keys: &SigningKeys,
    principal: Option<Uuid>,
    now: i64,
) -> Response {
    let Some(topics) = heard.filter(|topics| *topics != Topics::Of(BTreeSet::new())) else {
        return response;
    };
    let is_html = response
        .headers()
        .get(CONTENT_TYPE)
        .is_some_and(|value| value == HTML);
    if response.status() != StatusCode::OK || !is_html || caller == Caller::Json {
        return response;
    }
    let (parts, body) = response.into_parts();
    let Ok(collected) = body.collect().await;
    let Ok(mut html) = String::from_utf8(Vec::from(collected.to_bytes())) else {
        let error = Error::internal_message("a rendered screen is not UTF-8");
        return answer::failure(error, caller);
    };
    let is_page = html.starts_with("<!doctype html>");
    let sealed = Sealed {
        keys,
        principal,
        now,
    };
    let marker = marker(target, &topics, !is_page, &sealed);
    match html.rfind("</body>").filter(|_| is_page) {
        Some(end) => html.insert_str(end, &marker),
        None => html.push_str(&marker),
    }
    Response::from_parts(parts, Full::new(Bytes::from(html)))
}

struct Sealed<'k> {
    keys: &'k SigningKeys,
    principal: Option<Uuid>,
    now: i64,
}

/// Sealed for whom the page was shown, so a caller hears only the topics of a page it was shown,
/// never `*` at will nor what another principal's page reads.
fn marker(target: &Target, topics: &Topics, out_of_band: bool, sealed: &Sealed<'_>) -> String {
    let listened = match topics {
        Topics::Of(topics) if topics.len() <= TOPICS_LIMIT => topics
            .iter()
            .map(Topic::to_string)
            .collect::<Vec<_>>()
            .join(","),
        Topics::Of(_) | Topics::Every => "*".to_owned(),
    };
    let oob = if out_of_band {
        " hx-swap-oob=\"true\""
    } else {
        ""
    };
    let until = seal_until(sealed.now);
    let stream = format!(
        "{PATH}{}?topics={listened}&amp;{UNTIL}{until}",
        target.audience.name
    );
    let text = sealed_text(&stream.replace("&amp;", "&"), sealed.principal);
    let seal = sealed.keys.sign(Signed::Link, text.as_bytes());
    format!(
        "<div id=\"{MARKER}\" hidden{oob} data-stream=\"{stream}&amp;{SEAL}{seal}\" \
         data-fragments=\"{}\"></div>",
        target.admission.rules.live.join(" ")
    )
}

/// Also keys a live page's tag, so a page the browser kept never holds a seal that ran out.
pub(crate) fn seal_until(now: i64) -> i64 {
    (now + SEAL_LIFETIME) / SEAL_STEP * SEAL_STEP + SEAL_STEP
}

/// Apart from any signed link, whose text is a path.
fn sealed_text(stream: &str, principal: Option<Uuid>) -> String {
    match principal {
        Some(principal) => format!("live:{principal}:{stream}"),
        None => format!("live:anyone:{stream}"),
    }
}

/// A page shown to no one reads what anyone may.
fn unsealed<'r>(request: &'r Parts, sealed: &Sealed<'_>) -> Option<&'r str> {
    let (stream, seal) = request
        .uri
        .path_and_query()?
        .as_str()
        .rsplit_once(&format!("&{SEAL}"))?;
    let sealed_for = |holder| {
        let text = sealed_text(stream, holder);
        sealed.keys.verifies(Signed::Link, text.as_bytes(), seal)
    };
    let holds = sealed
        .principal
        .is_some_and(|_| sealed_for(sealed.principal))
        || sealed_for(None);
    let until: i64 = stream.rsplit_once(&format!("&{UNTIL}"))?.1.parse().ok()?;
    (holds && sealed.now < until).then_some(stream)
}

pub(crate) async fn answer(
    pipeline: &Pipeline,
    request: http::Request<Incoming>,
    peer: SocketAddr,
) -> http::Response<Body> {
    let is_stream = request.method() == Method::GET && request.uri().path().starts_with(PATH);
    if !is_stream {
        return pipeline.handle(request, peer).await.map(Either::Left);
    }
    let (request, _) = request.into_parts();
    #[cfg(feature = "shield")]
    if let Some(refusal) = pipeline.shielded(&request, peer) {
        return answer::secure(refusal).map(Either::Left);
    }
    match pipeline.live.stream(pipeline, &request, peer).await {
        Ok(events) => events,
        Err(refusal) => answer::secure(*refusal).map(Either::Left),
    }
}

impl Live {
    #[cfg_attr(
        not(feature = "session-list"),
        expect(
            clippy::unused_async_trait_impl,
            reason = "only a session store is asked"
        )
    )]
    async fn stream(
        &self,
        pipeline: &Pipeline,
        request: &Parts,
        peer: SocketAddr,
    ) -> Result<http::Response<Body>, Box<Response>> {
        let refuse = |status, text| Box::new(answer::response(status, Some(TEXT), text));
        let area = &request.uri.path()[PATH.len()..];
        let audience = self
            .areas
            .get(area)
            .ok_or_else(|| refuse(StatusCode::NOT_FOUND, "Not found."))?;
        if is_cross_site(request) {
            return Err(refuse(StatusCode::FORBIDDEN, "Refused."));
        }
        let now = pipeline.db.clock().now().timestamp();
        let session = pipeline.sessions.open(&request.headers, now);
        #[cfg(feature = "session-list")]
        let session = session
            .confirmed(&pipeline.sessions, &pipeline.db, now)
            .await
            .map_err(|error| Box::new(answer::failure(error, Caller::Json)))?;
        let reach = audience
            .reach(&session)
            .ok_or_else(|| refuse(StatusCode::UNAUTHORIZED, "Sign in first."))?;
        let sealed = Sealed {
            keys: &pipeline.sessions.keys,
            principal: session.principal(),
            now: now.as_second(),
        };
        let topics = unsealed(request, &sealed)
            .and_then(|stream| listened(stream.split_once('?').map(|(_, query)| query)))
            .ok_or_else(|| refuse(StatusCode::BAD_REQUEST, "Not a list of topics."))?;
        let holder = Requester::of(session.principal(), pipeline.client(request, peer).0);
        let admitted = self.admit(holder).ok_or_else(|| {
            let mut busy = refuse(StatusCode::TOO_MANY_REQUESTS, "Too many open streams.");
            busy.headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from_static(RETRY));
            busy
        })?;
        let (frames, events) = mpsc::channel(1);
        let listening = Listening {
            changes: sofie_db::committed_changes(&pipeline.db),
            topics,
            reach,
            closing: self.closing.subscribe(),
            _admitted: admitted,
        };
        tokio::spawn(listening.forward(frames));
        let head = answer::secure(answer::response(StatusCode::OK, None, Bytes::new()));
        let (mut parts, _) = head.into_parts();
        parts
            .headers
            .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
        parts
            .headers
            .insert("x-accel-buffering", HeaderValue::from_static("no"));
        Ok(http::Response::from_parts(
            parts,
            Either::Right(Events(events)),
        ))
    }
}

/// Unlike a page, a stream's opening is no write, so CSRF lets it pass; another site
/// opening one would still hold a place of the person's.
fn is_cross_site(request: &Parts) -> bool {
    request
        .headers
        .get("sec-fetch-site")
        .is_some_and(|site| site != "same-origin")
}

fn listened(query: Option<&str>) -> Option<Topics> {
    let list = query?
        .split('&')
        .find_map(|pair| pair.strip_prefix("topics="))?;
    if list == "*" {
        return Some(Topics::Every);
    }
    let topics: BTreeSet<Topic> = list.split(',').map(Topic::parse).collect::<Option<_>>()?;
    (topics.len() <= TOPICS_LIMIT).then_some(Topics::Of(topics))
}

struct Listening {
    changes: broadcast::Receiver<Announced>,
    topics: Topics,
    reach: Reach,
    closing: watch::Receiver<bool>,
    _admitted: Admitted,
}

enum Woken {
    Changed,
    Beat,
    Expired,
    Ended,
}

impl Listening {
    async fn forward(mut self, frames: mpsc::Sender<Bytes>) {
        let mut heartbeat = tokio::time::interval(HEARTBEAT);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        heartbeat.reset();
        let mut lifetime = pin!(tokio::time::sleep(LIFETIME));
        let mut frame = OPENED;
        loop {
            if frames.send(Bytes::from_static(frame)).await.is_err() || frame == END {
                return;
            }
            frame = match self.woken(&frames, &mut heartbeat, lifetime.as_mut()).await {
                Woken::Changed => {
                    tokio::time::sleep(SETTLE).await;
                    while self.changes.try_recv().is_ok() {}
                    CHANGED
                }
                Woken::Beat => BEAT,
                Woken::Expired => END,
                Woken::Ended => return,
            };
        }
    }

    async fn woken(
        &mut self,
        frames: &mpsc::Sender<Bytes>,
        heartbeat: &mut Interval,
        mut lifetime: Pin<&mut Sleep>,
    ) -> Woken {
        if *self.closing.borrow() {
            return Woken::Ended;
        }
        loop {
            let received = {
                let mut received = pin!(self.changes.recv());
                let mut closing = pin!(self.closing.changed());
                let mut left = pin!(frames.closed());
                let woken = poll_fn(|context: &mut Context<'_>| {
                    if lifetime.as_mut().poll(context).is_ready() {
                        return Poll::Ready(Err(Woken::Expired));
                    }
                    if closing.as_mut().poll(context).is_ready()
                        || left.as_mut().poll(context).is_ready()
                    {
                        return Poll::Ready(Err(Woken::Ended));
                    }
                    if let Poll::Ready(received) = received.as_mut().poll(context) {
                        return Poll::Ready(Ok(received));
                    }
                    heartbeat.poll_tick(context).map(|_| Err(Woken::Beat))
                })
                .await;
                match woken {
                    Ok(received) => received,
                    Err(woken) => return woken,
                }
            };
            match received {
                Ok(announced) if !announced.concerns(&self.topics, self.reach) => {}
                // A screen that fell behind may have missed a change it reads.
                Ok(_) | Err(RecvError::Lagged(_)) => return Woken::Changed,
                Err(RecvError::Closed) => return Woken::Ended,
            }
        }
    }
}

pub(crate) struct Events(mpsc::Receiver<Bytes>);

impl hyper::body::Body for Events {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        self.0
            .poll_recv(context)
            .map(|frame| frame.map(|bytes| Ok(Frame::data(bytes))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_listens_to_the_topics_its_page_names() {
        let topic = Topic::named("shop", "order");

        assert_eq!(listened(Some("topics=*")), Some(Topics::Every));
        assert_eq!(
            listened(Some(&format!("other=1&topics={topic}"))),
            Some(Topics::Of(BTreeSet::from([topic])))
        );
        assert_eq!(listened(Some("topics=nothex")), None);
        assert_eq!(listened(None), None);
        let too_many: Vec<String> = (0..=TOPICS_LIMIT)
            .map(|table| Topic::named("shop", &table.to_string()).to_string())
            .collect();
        assert_eq!(
            listened(Some(&format!("topics={}", too_many.join(",")))),
            None
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_stream_says_when_it_ends_for_its_lifetime() {
        let live = Live::new(&[]).unwrap();
        let (_announcer, changes) = broadcast::channel::<Announced>(1);
        let holder = Requester::of(None, std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        let listening = Listening {
            changes,
            topics: Topics::Every,
            reach: Reach::All,
            closing: live.closing.subscribe(),
            _admitted: live.admit(holder).unwrap(),
        };
        let (frames, mut events) = mpsc::channel(1);
        tokio::spawn(listening.forward(frames));

        let mut last = None;
        while let Some(frame) = events.recv().await {
            last = Some(frame);
        }

        assert_eq!(last.as_deref(), Some(END));
    }

    #[test]
    fn the_addresses_of_one_ipv6_64_share_one_holder_s_streams() {
        let live = Live::new(&[]).unwrap();
        let holder = |last: u16| {
            let address = std::net::Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, last);
            Requester::of(None, std::net::IpAddr::V6(address))
        };

        let admitted: Vec<Admitted> = (0..8).filter_map(|last| live.admit(holder(last))).collect();

        assert_eq!(admitted.len(), PER_HOLDER);
        assert!(live.admit(holder(9)).is_none());
    }
}
