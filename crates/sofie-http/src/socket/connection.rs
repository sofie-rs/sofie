use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::header::CONTENT_TYPE;
use http::uri::PathAndQuery;
use http::{HeaderMap, Method, StatusCode};
use http_body_util::BodyExt;
use sofie_db::{Announced, Reach, Topics};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::{Instant, MissedTickBehavior};

use super::frames::{self, CloseCode, ReadError, Reader, Received};
use super::protocol::{self, Message, Verb};
use crate::holders::Held;
use crate::pipeline::Pipeline;
use crate::router::Lookup;
use crate::session::RequestSession;

/// Proxies close a connection idle for a minute or so.
const HEARTBEAT: Duration = Duration::from_secs(20);
/// A browser answers each ping, so this much silence is a client gone.
const IDLE: Duration = Duration::from_mins(1);
/// The cookie is the handshake's, so a session that ended stops hearing once the client
/// reopens with its current one.
const LIFETIME: Duration = Duration::from_mins(10);
const SETTLE: Duration = Duration::from_millis(50);
const WATCH_LIMIT: usize = 32;
/// Past it the socket is not read until an answer finishes.
const IN_FLIGHT: usize = 8;
const OUTBOX: usize = 16;
const HEARD_LIMIT: usize = 64;

pub(crate) struct Link {
    pub(crate) pipeline: Arc<Pipeline>,
    pub(crate) peer: SocketAddr,
    pub(crate) headers: HeaderMap,
    pub(crate) session: RequestSession,
    pub(crate) closing: watch::Receiver<bool>,
    pub(crate) _held: Held,
    pub(crate) _connection: Option<super::Hold>,
}

pub(crate) async fn run<S>(io: S, link: Link)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (reading, writing) = tokio::io::split(io);
    let (outbox, outgoing) = mpsc::channel(OUTBOX);
    let (heard, inbox) = mpsc::channel(1);
    let writer = tokio::spawn(write(writing, outgoing));
    let reader = tokio::spawn(read(reading, heard, outbox.clone()));
    let changes = sofie_db::committed_changes(&link.pipeline.db);
    let connection = Connection {
        link,
        outbox,
        watches: HashMap::new(),
        running: JoinSet::new(),
        changes,
        settles_at: None,
    };
    connection.serve(inbox).await;
    reader.abort();
    if let Err(error) = writer.await {
        tracing::debug!(error = %error, "a socket's writer ended abruptly");
    }
}

async fn write<S: AsyncWrite>(mut writing: WriteHalf<S>, mut outgoing: mpsc::Receiver<Bytes>) {
    while let Some(frame) = outgoing.recv().await {
        let is_close = frame.first() == Some(&0x88);
        if writing.write_all(&frame).await.is_err() || writing.flush().await.is_err() {
            return;
        }
        if is_close {
            let _ = writing.shutdown().await;
            return;
        }
    }
}

enum Heard {
    Message(String),
    /// Ends the socket with this code; `None` when the client is gone or closed it.
    Ended(Option<CloseCode>),
}

async fn read<S: AsyncRead>(
    reading: ReadHalf<S>,
    heard: mpsc::Sender<Heard>,
    outbox: mpsc::Sender<Bytes>,
) {
    let mut reader = Reader::new(reading);
    loop {
        let ended = match tokio::time::timeout(IDLE, reader.next()).await {
            Ok(Ok(Received::Text(text))) => {
                if heard.send(Heard::Message(text)).await.is_err() {
                    return;
                }
                continue;
            }
            Ok(Ok(Received::Ping(payload))) => {
                if outbox.send(frames::pong(&payload)).await.is_err() {
                    return;
                }
                continue;
            }
            Ok(Ok(Received::Pong)) => continue,
            Ok(Ok(Received::Close)) => Some(CloseCode::Normal),
            Ok(Err(ReadError::Violated(code))) => Some(code),
            Ok(Err(ReadError::Gone)) | Err(_) => None,
        };
        let _ = heard.send(Heard::Ended(ended)).await;
        return;
    }
}

struct Watch {
    path: PathAndQuery,
    reach: Reach,
    topics: Topics,
    /// Tells a stopped or replaced watch's late answer apart.
    generation: u64,
    sent: Option<u64>,
    run: Run,
}

enum Run {
    Idle { due: bool },
    Running { heard: Vec<Announced> },
}

struct Answered {
    id: u64,
    watched: Option<u64>,
    status: StatusCode,
    text: String,
    digest: u64,
    topics: Topics,
}

struct Connection {
    link: Link,
    outbox: mpsc::Sender<Bytes>,
    watches: HashMap<u64, Watch>,
    running: JoinSet<Answered>,
    changes: tokio::sync::broadcast::Receiver<Announced>,
    settles_at: Option<Instant>,
}

enum Step {
    Go,
    End(CloseCode, &'static str),
    Gone,
}

impl Connection {
    async fn serve(mut self, mut inbox: mpsc::Receiver<Heard>) {
        let mut heartbeat = tokio::time::interval(HEARTBEAT);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        heartbeat.reset();
        let mut lifetime = pin!(tokio::time::sleep(LIFETIME));
        let mut closing = self.link.closing.clone();
        let mut generation = 0;
        loop {
            let settles_at = self.settles_at.unwrap_or_else(Instant::now);
            let step = tokio::select! {
                biased;
                _ = async { closing.wait_for(|closing| *closing).await.is_ok() } => {
                    Step::End(CloseCode::GoingAway, "stopping")
                }
                () = &mut lifetime => Step::End(CloseCode::Reconnect, "reopen"),
                Some(answered) = self.running.join_next() => match answered {
                    Ok(answered) => self.answered(answered).await,
                    Err(error) => {
                        tracing::error!(error.kind = "socket", error = %error, "a socket's call ended abruptly");
                        Step::Go
                    }
                },
                // Before changes, so a steady stream of them cannot hold answers back.
                () = tokio::time::sleep_until(settles_at), if self.settles_at.is_some() => {
                    self.settles_at = None;
                    self.start_due();
                    Step::Go
                }
                heard = inbox.recv(), if self.running.len() < IN_FLIGHT => match heard {
                    Some(Heard::Message(text)) => {
                        generation += 1;
                        self.asked(&text, generation).await
                    }
                    Some(Heard::Ended(Some(code))) => Step::End(code, ""),
                    Some(Heard::Ended(None)) | None => Step::Gone,
                },
                received = self.changes.recv() => self.heard(received),
                _ = heartbeat.tick() => self.send(frames::ping()).await,
            };
            match step {
                Step::Go => {}
                Step::End(code, reason) => {
                    let _ = self.outbox.send(frames::close(code, reason)).await;
                    break;
                }
                Step::Gone => break,
            }
        }
        // A call the client sent still runs to its end, so none stops halfway.
        self.running.detach_all();
    }

    async fn send(&self, frame: Bytes) -> Step {
        match self.outbox.send(frame).await {
            Ok(()) => Step::Go,
            Err(_) => Step::Gone,
        }
    }

    async fn asked(&mut self, text: &str, generation: u64) -> Step {
        let Ok(message) = serde_json::from_str::<Message>(text) else {
            return Step::End(CloseCode::Policy, "not a message");
        };
        let (id, verb, path, body) = match message {
            Message::Stop { stop } => {
                self.watches.remove(&stop);
                return Step::Go;
            }
            Message::Call {
                id,
                method,
                path,
                body,
            } => (id, method, path, body),
        };
        let Some(path) = protocol::path(&path) else {
            return self
                .send_text(protocol::refused(
                    id,
                    StatusCode::BAD_REQUEST,
                    "Not a path of this app.",
                ))
                .await;
        };
        if verb != Verb::Get {
            self.start(id, None, verb, path, body.as_ref());
            return Step::Go;
        }
        if !self.watches.contains_key(&id) && self.watches.len() >= WATCH_LIMIT {
            let text = protocol::refused(
                id,
                StatusCode::TOO_MANY_REQUESTS,
                "Too many watches on this socket.",
            );
            return self.send_text(text).await;
        }
        let watch = Watch {
            reach: self.reach(&path),
            path: path.clone(),
            topics: Topics::Of(std::collections::BTreeSet::new()),
            generation,
            sent: None,
            run: Run::Running { heard: Vec::new() },
        };
        self.watches.insert(id, watch);
        self.start(id, Some(generation), Verb::Get, path, None);
        Step::Go
    }

    fn reach(&self, path: &PathAndQuery) -> Reach {
        match self.link.pipeline.router.find(&Method::GET, path.path()) {
            Lookup::Found(target, _) => target
                .audience
                .reach(&self.link.session)
                .unwrap_or(Reach::All),
            Lookup::WrongMethod(_) | Lookup::Missing => Reach::All,
        }
    }

    fn start(
        &mut self,
        id: u64,
        watched: Option<u64>,
        verb: Verb,
        path: PathAndQuery,
        body: Option<&serde_json::Value>,
    ) {
        let request = protocol::request(&self.link.headers, verb, path, body);
        let pipeline = Arc::clone(&self.link.pipeline);
        let peer = self.link.peer;
        self.running.spawn(async move {
            let (response, topics) = sofie_db::track_topics(pipeline.answer(request, peer)).await;
            let (parts, body) = response.into_parts();
            let Ok(body) = body.collect().await;
            let body = body.to_bytes();
            let text = protocol::reply(id, parts.status, parts.headers.get(CONTENT_TYPE), &body);
            let digest = sofie_core::Fnv1a64::new()
                .write(&parts.status.as_u16().to_be_bytes())
                .write(&body)
                .finish();
            Answered {
                id,
                watched,
                status: parts.status,
                text,
                digest,
                topics,
            }
        });
    }

    async fn answered(&mut self, answered: Answered) -> Step {
        self.start_due();
        let Some(generation) = answered.watched else {
            return self.send_text(answered.text).await;
        };
        let Some(watch) = self
            .watches
            .get_mut(&answered.id)
            .filter(|watch| watch.generation == generation)
        else {
            return Step::Go;
        };
        let Run::Running { heard } = std::mem::replace(&mut watch.run, Run::Idle { due: false })
        else {
            return Step::Go;
        };
        // An answer that is not a success ends its watch: it may be a refusal to this caller.
        if !answered.status.is_success() {
            self.watches.remove(&answered.id);
            return self.send_text(answered.text).await;
        }
        let due = heard
            .iter()
            .any(|change| change.concerns(&answered.topics, watch.reach));
        watch.run = Run::Idle { due };
        watch.topics = answered.topics;
        let unchanged = watch.sent == Some(answered.digest);
        watch.sent = Some(answered.digest);
        if due {
            self.settle();
        }
        if unchanged {
            return Step::Go;
        }
        self.send_text(answered.text).await
    }

    fn heard(&mut self, received: Result<Announced, RecvError>) -> Step {
        let announced = match received {
            Ok(announced) => announced,
            // A socket that fell behind may have missed a change it watches.
            Err(RecvError::Lagged(_)) => Announced::MaybeMissed,
            Err(RecvError::Closed) => return Step::End(CloseCode::GoingAway, "stopping"),
        };
        let mut concerned = false;
        for watch in self.watches.values_mut() {
            match &mut watch.run {
                Run::Idle { due } => {
                    if announced.concerns(&watch.topics, watch.reach) {
                        *due = true;
                        concerned = true;
                    }
                }
                Run::Running { heard } if heard.len() < HEARD_LIMIT => {
                    heard.push(announced.clone());
                }
                Run::Running { heard } => *heard = vec![Announced::MaybeMissed],
            }
        }
        if concerned {
            self.settle();
        }
        Step::Go
    }

    fn settle(&mut self) {
        self.settles_at
            .get_or_insert_with(|| Instant::now() + SETTLE);
    }

    fn start_due(&mut self) {
        if self.settles_at.is_some() {
            return;
        }
        let due: Vec<(u64, u64, PathAndQuery)> = self
            .watches
            .iter()
            .filter(|(_, watch)| matches!(watch.run, Run::Idle { due: true }))
            .map(|(id, watch)| (*id, watch.generation, watch.path.clone()))
            .take(IN_FLIGHT.saturating_sub(self.running.len()))
            .collect();
        for (id, generation, path) in due {
            if let Some(watch) = self.watches.get_mut(&id) {
                watch.run = Run::Running { heard: Vec::new() };
            }
            self.start(id, Some(generation), Verb::Get, path, None);
        }
    }

    async fn send_text(&self, text: String) -> Step {
        self.send(frames::text(&text)).await
    }
}
