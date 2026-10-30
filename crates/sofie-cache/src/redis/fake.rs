//! A Redis server on a local socket speaking RESP2, written apart from the
//! client's own parser so the two cannot share a mistake.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::{Buf, BytesMut};
use sofie_core::unpoisoned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

enum Value {
    Text(Vec<u8>),
    Sorted(HashMap<Vec<u8>, i64>),
}

#[derive(Default)]
struct Data {
    values: HashMap<Vec<u8>, (Value, Option<Instant>)>,
    subscribers: Vec<mpsc::UnboundedSender<Vec<u8>>>,
    selected: Vec<String>,
    transactions: Vec<Vec<String>>,
}

struct Shared {
    password: Option<String>,
    data: Mutex<Data>,
    connections: AtomicUsize,
    /// Commands answered before every later one is refused, as when Redis goes away mid-drop.
    answering: AtomicUsize,
    closing: broadcast::Sender<()>,
}

pub(crate) struct FakeRedis {
    pub(crate) port: u16,
    shared: Arc<Shared>,
    accepting: JoinHandle<()>,
}

impl FakeRedis {
    pub(crate) async fn start(password: Option<&str>, tls: Option<TlsAcceptor>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let shared = Arc::new(Shared {
            password: password.map(str::to_owned),
            data: Mutex::default(),
            connections: AtomicUsize::new(0),
            answering: AtomicUsize::new(usize::MAX),
            closing: broadcast::channel(1).0,
        });
        let serving = Arc::clone(&shared);
        let accepting = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                serving.connections.fetch_add(1, Ordering::SeqCst);
                let (shared, tls) = (Arc::clone(&serving), tls.clone());
                tokio::spawn(async move {
                    match tls {
                        Some(tls) => {
                            if let Ok(stream) = tls.accept(stream).await {
                                serve(stream, shared).await;
                            }
                        }
                        None => serve(stream, shared).await,
                    }
                });
            }
        });
        Self {
            port,
            shared,
            accepting,
        }
    }

    pub(crate) fn connections(&self) -> usize {
        self.shared.connections.load(Ordering::SeqCst)
    }

    pub(crate) fn selected(&self) -> Vec<String> {
        self.data().selected.clone()
    }

    pub(crate) fn transactions(&self) -> Vec<Vec<String>> {
        self.data().transactions.clone()
    }

    pub(crate) fn holds(&self, key: &str) -> bool {
        self.data().values.contains_key(key.as_bytes())
    }

    pub(crate) fn members(&self, key: &str) -> Vec<String> {
        let data = self.data();
        let Some((Value::Sorted(members), _)) = data.values.get(key.as_bytes()) else {
            return Vec::new();
        };
        let mut members: Vec<String> = members
            .keys()
            .map(|member| String::from_utf8_lossy(member).into_owned())
            .collect();
        members.sort();
        members
    }

    pub(crate) fn refuse_after(&self, commands: usize) {
        self.shared.answering.store(commands, Ordering::SeqCst);
    }

    pub(crate) fn answer_all(&self) {
        self.refuse_after(usize::MAX);
    }

    pub(crate) fn close_connections(&self) {
        let _ = self.shared.closing.send(());
    }

    pub(crate) fn stop(&self) {
        self.accepting.abort();
        self.close_connections();
    }

    fn data(&self) -> std::sync::MutexGuard<'_, Data> {
        unpoisoned::lock(&self.shared.data)
    }
}

async fn serve(mut stream: impl AsyncRead + AsyncWrite + Unpin, shared: Arc<Shared>) {
    let mut closing = shared.closing.subscribe();
    let (messages, mut published) = mpsc::unbounded_channel();
    let mut session = Session {
        signed_in: shared.password.is_none(),
        queued: None,
        messages,
        subscribed: false,
    };
    let mut buffer = BytesMut::new();
    loop {
        while let Some(command) = command(&mut buffer) {
            let refused = shared
                .answering
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    left.checked_sub(1)
                })
                .is_err();
            let reply = if refused {
                error("ERR refused by the test")
            } else {
                session.answer(&shared, &command)
            };
            if stream.write_all(&reply).await.is_err() {
                return;
            }
        }
        tokio::select! {
            read = stream.read_buf(&mut buffer) => {
                if !matches!(read, Ok(1..)) {
                    return;
                }
            }
            Some(message) = published.recv() => {
                if stream.write_all(&message).await.is_err() {
                    return;
                }
            }
            _ = closing.recv() => return,
        }
    }
}

fn command(buffer: &mut BytesMut) -> Option<Vec<Vec<u8>>> {
    let text = &buffer[..];
    let line_end = |from: usize| {
        text.get(from..)?
            .windows(2)
            .position(|pair| pair == b"\r\n")
            .map(|at| from + at)
    };
    let end = line_end(0)?;
    let count: usize = std::str::from_utf8(&text[1..end]).ok()?.parse().ok()?;
    let mut at = end + 2;
    let mut arguments = Vec::with_capacity(count);
    for _ in 0..count {
        let end = line_end(at)?;
        let length: usize = std::str::from_utf8(&text[at + 1..end]).ok()?.parse().ok()?;
        let start = end + 2;
        arguments.push(text.get(start..start + length)?.to_vec());
        at = start + length + 2;
        if text.len() < at {
            return None;
        }
    }
    buffer.advance(at);
    Some(arguments)
}

struct Session {
    signed_in: bool,
    queued: Option<Vec<Vec<Vec<u8>>>>,
    messages: mpsc::UnboundedSender<Vec<u8>>,
    subscribed: bool,
}

fn status(text: &str) -> Vec<u8> {
    format!("+{text}\r\n").into_bytes()
}

fn error(text: &str) -> Vec<u8> {
    format!("-{text}\r\n").into_bytes()
}

fn integer(value: i128) -> Vec<u8> {
    format!(":{value}\r\n").into_bytes()
}

fn bulk(value: Option<&[u8]>) -> Vec<u8> {
    let Some(value) = value else {
        return b"$-1\r\n".to_vec();
    };
    let mut out = format!("${}\r\n", value.len()).into_bytes();
    out.extend_from_slice(value);
    out.extend_from_slice(b"\r\n");
    out
}

fn array(items: Vec<Vec<u8>>) -> Vec<u8> {
    let mut out = format!("*{}\r\n", items.len()).into_bytes();
    for item in items {
        out.extend(item);
    }
    out
}

impl Session {
    fn answer(&mut self, shared: &Shared, command: &[Vec<u8>]) -> Vec<u8> {
        let name = String::from_utf8_lossy(&command[0]).to_ascii_uppercase();
        let arguments = &command[1..];
        if name == "AUTH" {
            let given = String::from_utf8_lossy(arguments.last().unwrap()).into_owned();
            self.signed_in = shared.password.as_deref() == Some(given.as_str());
            return if self.signed_in {
                status("OK")
            } else {
                error("WRONGPASS invalid username-password pair")
            };
        }
        if !self.signed_in {
            return error("NOAUTH Authentication required.");
        }
        if let Some(queued) = &mut self.queued
            && name != "EXEC"
        {
            queued.push(command.to_vec());
            return status("QUEUED");
        }
        match name.as_str() {
            "MULTI" => {
                self.queued = Some(Vec::new());
                status("OK")
            }
            "EXEC" => {
                let queued = self.queued.take().unwrap_or_default();
                let names = queued
                    .iter()
                    .map(|command| String::from_utf8_lossy(&command[0]).to_ascii_uppercase())
                    .collect();
                unpoisoned::lock(&shared.data).transactions.push(names);
                array(
                    queued
                        .iter()
                        .map(|command| self.answer(shared, command))
                        .collect(),
                )
            }
            "SUBSCRIBE" => {
                self.subscribed = true;
                let mut data = unpoisoned::lock(&shared.data);
                data.subscribers.push(self.messages.clone());
                array(vec![
                    bulk(Some(b"subscribe")),
                    bulk(Some(&arguments[0])),
                    integer(1),
                ])
            }
            "PING" if self.subscribed => array(vec![bulk(Some(b"pong")), bulk(Some(b""))]),
            _ => {
                let mut data = unpoisoned::lock(&shared.data);
                data.answer(&name, arguments)
            }
        }
    }
}

impl Data {
    fn live(&mut self, key: &[u8]) -> Option<&mut (Value, Option<Instant>)> {
        let expired = self
            .values
            .get(key)?
            .1
            .is_some_and(|until| until <= Instant::now());
        if expired {
            self.values.remove(key);
            return None;
        }
        self.values.get_mut(key)
    }

    fn answer(&mut self, name: &str, arguments: &[Vec<u8>]) -> Vec<u8> {
        let number =
            |at: usize| -> u64 { String::from_utf8_lossy(&arguments[at]).parse().unwrap() };
        match name {
            "PING" => status("PONG"),
            "SELECT" => {
                self.selected
                    .push(String::from_utf8_lossy(&arguments[0]).into_owned());
                status("OK")
            }
            "GET" => match self.live(&arguments[0]) {
                Some((Value::Text(text), _)) => bulk(Some(text)),
                Some(_) => error("WRONGTYPE"),
                None => bulk(None),
            },
            "PTTL" => match self.live(&arguments[0]) {
                Some((_, Some(until))) => integer(
                    until
                        .saturating_duration_since(Instant::now())
                        .as_millis()
                        .try_into()
                        .unwrap(),
                ),
                Some((_, None)) => integer(-1),
                None => integer(-2),
            },
            "SET" => {
                let until = match arguments.get(2).map(|unit| unit.to_ascii_uppercase()) {
                    Some(unit) if unit == b"PX" => Some(Duration::from_millis(number(3))),
                    Some(unit) if unit == b"EX" => Some(Duration::from_secs(number(3))),
                    _ => None,
                };
                let until = until.map(|left| Instant::now() + left);
                self.values.insert(
                    arguments[0].clone(),
                    (Value::Text(arguments[1].clone()), until),
                );
                status("OK")
            }
            "DEL" => integer(
                arguments
                    .iter()
                    .filter(|key| self.values.remove(*key).is_some())
                    .count()
                    .try_into()
                    .unwrap(),
            ),
            "ZADD" | "ZREMRANGEBYSCORE" | "ZRANGE" | "ZREM" => self.sorted(name, arguments),
            "PEXPIRE" => {
                let left = Duration::from_millis(number(1));
                match self.live(&arguments[0]) {
                    Some((_, until)) => {
                        *until = Some(Instant::now() + left);
                        integer(1)
                    }
                    None => integer(0),
                }
            }
            "PUBLISH" => {
                let message = array(vec![
                    bulk(Some(b"message")),
                    bulk(Some(&arguments[0])),
                    bulk(Some(&arguments[1])),
                ]);
                self.subscribers
                    .retain(|subscriber| subscriber.send(message.clone()).is_ok());
                integer(self.subscribers.len().try_into().unwrap())
            }
            other => error(&format!("ERR unknown command '{other}'")),
        }
    }

    fn sorted(&mut self, name: &str, arguments: &[Vec<u8>]) -> Vec<u8> {
        let number =
            |at: usize| -> i64 { String::from_utf8_lossy(&arguments[at]).parse().unwrap() };
        match name {
            "ZADD" => {
                if self.live(&arguments[0]).is_none() {
                    self.values
                        .insert(arguments[0].clone(), (Value::Sorted(HashMap::new()), None));
                }
                let score = number(1);
                let Some((Value::Sorted(members), _)) = self.live(&arguments[0]) else {
                    return error("WRONGTYPE");
                };
                integer(i128::from(
                    members.insert(arguments[2].clone(), score).is_none(),
                ))
            }
            "ZREMRANGEBYSCORE" => {
                let below = number(2);
                let Some((Value::Sorted(members), _)) = self.live(&arguments[0]) else {
                    return integer(0);
                };
                let before = members.len();
                members.retain(|_, score| *score > below);
                integer((before - members.len()).try_into().unwrap())
            }
            "ZRANGE" => {
                let count = usize::try_from(number(2) + 1).unwrap();
                let Some((Value::Sorted(members), _)) = self.live(&arguments[0]) else {
                    return array(Vec::new());
                };
                let mut lowest: Vec<(&Vec<u8>, &i64)> = members.iter().collect();
                lowest.sort_by_key(|(_, score)| **score);
                lowest.truncate(count);
                array(
                    lowest
                        .into_iter()
                        .map(|(member, _)| bulk(Some(member)))
                        .collect(),
                )
            }
            "ZREM" => {
                let Some((Value::Sorted(members), _)) = self.live(&arguments[0]) else {
                    return integer(0);
                };
                let removed = arguments[1..]
                    .iter()
                    .filter(|member| members.remove(*member).is_some())
                    .count();
                if members.is_empty() {
                    self.values.remove(&arguments[0]);
                }
                integer(removed.try_into().unwrap())
            }
            other => error(&format!("ERR unknown command '{other}'")),
        }
    }
}
