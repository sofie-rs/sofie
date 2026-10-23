//! The few commands a cache needs, without a `redis` crate.

mod connection;
#[cfg(test)]
mod fake;
mod resp;
#[cfg(test)]
mod tests;
mod url;

use std::sync::Arc;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use sofie_core::{Error, Result, unpoisoned};
use sofie_db::BoxFuture;
#[cfg(feature = "memory")]
use sofie_db::Notifications;
use tokio::sync::Semaphore;
use tokio::time::{Instant, timeout};

use crate::MAX_EXPIRY;
use crate::backoff::Backoff;
use crate::shared::{SharedStore, Stored, milliseconds};
use connection::{Connection, Target, checked};
use resp::{Command, Reply};
pub use url::RedisUrl;

const MOST_CONNECTIONS: usize = 16;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(feature = "memory")]
/// A subscription quiet this long is pinged, and dropped when the ping goes unanswered as long.
const SUBSCRIBED_IDLE: Duration = Duration::from_secs(30);
/// Sorted sets of the keys under a tag, each scored by when its entry expires.
const TAG_PREFIX: &str = "sofie:tags:";
/// A member is pruned this long after its entry expires, so instances' clocks may disagree by it.
const CLOCK_SKEW: Duration = Duration::from_mins(1);
/// Members popped per tag per round trip, so no drop moves one huge reply.
const DROP_CHUNK: usize = 512;

pub(crate) struct Client {
    target: Target,
    idle: Mutex<Vec<Connection>>,
    permits: Semaphore,
    reconnect: Mutex<Reconnect>,
}

#[derive(Default)]
struct Reconnect {
    backoff: Backoff,
    not_before: Option<Instant>,
}

impl Client {
    /// Fails when Redis cannot be reached, so a misconfigured app fails at boot.
    pub(crate) async fn connect(url: RedisUrl) -> Result<Self> {
        Self::reaching(Target::new(url)).await
    }

    async fn reaching(target: Target) -> Result<Self> {
        let client = Self {
            target,
            idle: Mutex::default(),
            permits: Semaphore::new(MOST_CONNECTIONS),
            reconnect: Mutex::default(),
        };
        client.call(&[Command::new("PING")]).await?;
        Ok(client)
    }

    fn idle(&self) -> MutexGuard<'_, Vec<Connection>> {
        unpoisoned::lock(&self.idle)
    }

    fn reconnect(&self) -> MutexGuard<'_, Reconnect> {
        unpoisoned::lock(&self.reconnect)
    }

    /// Pipelined on one connection; an idle connection that fails is replaced once, as Redis
    /// may have closed it meanwhile.
    pub(crate) async fn call(&self, commands: &[Command]) -> Result<Vec<Reply>> {
        let _permit = timeout(COMMAND_TIMEOUT, self.permits.acquire())
            .await
            .map_err(|_| {
                Error::Unavailable(format!(
                    "all {MOST_CONNECTIONS} Redis connections stayed busy for {} ms",
                    COMMAND_TIMEOUT.as_millis()
                ))
            })?
            .map_err(|error| Error::internal("waiting for a Redis connection", error))?;
        let reused = self.idle().pop();
        if let Some(connection) = reused
            && let Ok(replies) = self.exchange(connection, commands).await
        {
            return checked(commands, replies);
        }
        let connection = self.open().await?;
        let replies = self.exchange(connection, commands).await?;
        checked(commands, replies)
    }

    /// Redis runs the commands together, with no other client's command between them.
    async fn transaction(&self, commands: Vec<Command>) -> Result<Vec<Reply>> {
        let mut wrapped = Vec::with_capacity(commands.len() + 2);
        wrapped.push(Command::new("MULTI"));
        wrapped.extend(commands);
        wrapped.push(Command::new("EXEC"));
        let replies = self.call(&wrapped).await?;
        let executed = replies.into_iter().last().and_then(Reply::into_items);
        let executed =
            executed.ok_or_else(|| Error::internal_message("Redis did not run a transaction"))?;
        checked(&wrapped[1..], executed)
    }

    async fn exchange(
        &self,
        mut connection: Connection,
        commands: &[Command],
    ) -> Result<Vec<Reply>> {
        match timeout(COMMAND_TIMEOUT, connection.call(commands)).await {
            Ok(Ok(replies)) => {
                self.idle().push(connection);
                Ok(replies)
            }
            Ok(Err(error)) => Err(error),
            Err(_) => Err(Error::Unavailable(format!(
                "Redis did not answer {} within {} ms",
                commands.first().map(Command::name).unwrap_or_default(),
                COMMAND_TIMEOUT.as_millis()
            ))),
        }
    }

    async fn open(&self) -> Result<Connection> {
        if let Some(not_before) = self.reconnect().not_before {
            let left = not_before.saturating_duration_since(Instant::now());
            if !left.is_zero() {
                return Err(Error::Unavailable(format!(
                    "Redis was unreachable; connecting again in {} ms",
                    left.as_millis()
                )));
            }
        }
        let opened = timeout(CONNECT_TIMEOUT, Connection::open(&self.target))
            .await
            .unwrap_or_else(|_| {
                Err(Error::Unavailable(format!(
                    "connecting to Redis took over {} ms",
                    CONNECT_TIMEOUT.as_millis()
                )))
            });
        let mut reconnect = self.reconnect();
        if opened.is_ok() {
            reconnect.backoff.succeeded();
            reconnect.not_before = None;
        } else {
            let wait = reconnect.backoff.failed();
            reconnect.not_before = Some(Instant::now() + wait);
        }
        opened
    }

    /// Pub/sub ignores the database number, so the channel names it: apps kept apart by database
    /// do not drop each other's memory copies.
    #[cfg(feature = "memory")]
    fn channel(&self, channel: &str) -> String {
        format!("{channel}:{}", self.target.url.database)
    }

    #[cfg(feature = "memory")]
    pub(crate) async fn publish(&self, channel: &str, payload: &str) -> Result<()> {
        let publish = Command::new("PUBLISH")
            .argument(self.channel(channel))
            .argument(payload);
        self.call(&[publish]).await.map(drop)
    }

    #[cfg(feature = "memory")]
    /// On a connection of its own, as a subscribed connection takes no other commands.
    pub(crate) async fn subscribe(&self, channel: &str) -> Result<Box<dyn Notifications>> {
        let opening = timeout(CONNECT_TIMEOUT, Connection::open(&self.target));
        let mut connection = opening.await.map_err(|_| {
            Error::Unavailable("connecting to Redis to subscribe timed out".to_owned())
        })??;
        connection
            .send(&[Command::new("SUBSCRIBE").argument(self.channel(channel))])
            .await?;
        let confirmed = timeout(COMMAND_TIMEOUT, connection.read())
            .await
            .map_err(|_| Error::Unavailable("Redis did not confirm a subscription".to_owned()))??;
        if pushed_kind(&confirmed).as_deref() != Some("subscribe") {
            return Err(Error::internal_message(format!(
                "Redis answered SUBSCRIBE with {confirmed:?}"
            )));
        }
        Ok(Box::new(Subscriber {
            connection,
            pinged: false,
        }))
    }
}

fn tag_key(tag: &str) -> String {
    format!("{TAG_PREFIX}{tag}")
}

/// The tags on the first line, so a read can hold the entry in memory under them too.
fn envelope(stored: &Stored) -> String {
    format!("{}\n{}", stored.tags.join(" "), stored.value)
}

fn opened(envelope: &[u8], left: Duration) -> Result<Stored> {
    let text = std::str::from_utf8(envelope)
        .map_err(|error| Error::internal("reading a cache entry from Redis", error))?;
    let (tags, value) = text
        .split_once('\n')
        .ok_or_else(|| Error::internal_message("a cache entry in Redis lacks its tag line"))?;
    Ok(Stored {
        value: Arc::from(value),
        tags: tags.split_whitespace().map(str::to_owned).collect(),
        expires_in: left,
    })
}

impl SharedStore for Client {
    fn name(&self) -> &'static str {
        "redis"
    }

    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Option<Stored>>> {
        Box::pin(async move {
            let commands = [
                Command::new("GET").argument(key),
                Command::new("PTTL").argument(key),
            ];
            let mut replies = self.call(&commands).await?.into_iter();
            let (Some(Reply::Bulk(found)), Some(Reply::Integer(left))) =
                (replies.next(), replies.next())
            else {
                return Err(Error::internal_message("Redis answered GET and PTTL oddly"));
            };
            // Gone between the two commands, or kept without expiry by someone else.
            let (Some(envelope), Ok(left @ 1..)) = (found, u64::try_from(left)) else {
                return Ok(None);
            };
            opened(&envelope, Duration::from_millis(left)).map(Some)
        })
    }

    /// One transaction, so no drop of a tag runs between the entry and its tag sets.
    fn set<'a>(&'a self, key: &'a str, stored: &'a Stored) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let now = unix_milliseconds()?;
            let expires_at = now.saturating_add(milliseconds(stored.expires_in));
            let mut commands = vec![
                Command::new("SET")
                    .argument(key)
                    .argument(envelope(stored))
                    .argument("PX")
                    .argument(milliseconds(stored.expires_in).to_string()),
            ];
            for tag in &stored.tags {
                commands.extend(tagging(&tag_key(tag), key, expires_at, now));
            }
            self.transaction(commands).await.map(drop)
        })
    }

    fn delete<'a>(&'a self, keys: &'a [String]) -> BoxFuture<'a, Result<()>> {
        let delete = keys.iter().fold(Command::new("DEL"), |delete, key| {
            delete.argument(key.as_str())
        });
        Box::pin(async move { self.call(&[delete]).await.map(drop) })
    }

    /// Each chunk is read, then deleted with its members in one transaction: a failure between
    /// the two loses no key, and a key added meanwhile is dropped too or stays for the next drop.
    fn drop_tagged<'a>(&'a self, tags: &'a [String]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut left: Vec<String> = tags.iter().map(|tag| tag_key(tag)).collect();
            while !left.is_empty() {
                left = self.drop_chunk(left).await?;
            }
            Ok(())
        })
    }
}

impl Client {
    /// Answers the tag sets that may hold more members.
    async fn drop_chunk(&self, tag_keys: Vec<String>) -> Result<Vec<String>> {
        let reads: Vec<Command> = tag_keys
            .iter()
            .map(|tag| {
                Command::new("ZRANGE")
                    .argument(tag.as_str())
                    .argument("0")
                    .argument((DROP_CHUNK - 1).to_string())
            })
            .collect();
        let read = self.call(&reads).await?;
        let mut more = Vec::new();
        let mut deleted = Vec::new();
        let mut removals = Vec::new();
        for (tag, reply) in tag_keys.into_iter().zip(read) {
            let members: Vec<Vec<u8>> = reply
                .into_items()
                .unwrap_or_default()
                .into_iter()
                .filter_map(Reply::into_bytes)
                .collect();
            if members.is_empty() {
                continue;
            }
            let removal = Command::new("ZREM").argument(tag.as_str());
            removals.push(
                members
                    .iter()
                    .fold(removal, |remove, member| remove.argument(member.as_slice())),
            );
            if members.len() == DROP_CHUNK {
                more.push(tag);
            }
            deleted.extend(members);
        }
        if deleted.is_empty() {
            return Ok(more);
        }
        let mut commands = vec![
            deleted
                .into_iter()
                .fold(Command::new("DEL"), Command::argument),
        ];
        commands.extend(removals);
        self.transaction(commands).await?;
        Ok(more)
    }
}

fn tagging(tag: &str, key: &str, expires_at: i64, now: i64) -> [Command; 3] {
    let skew = milliseconds(CLOCK_SKEW);
    [
        Command::new("ZADD")
            .argument(tag)
            .argument(expires_at.saturating_add(skew).to_string())
            .argument(key),
        Command::new("ZREMRANGEBYSCORE")
            .argument(tag)
            .argument("-inf")
            .argument(now.to_string()),
        Command::new("PEXPIRE")
            .argument(tag)
            .argument(milliseconds(MAX_EXPIRY + CLOCK_SKEW).to_string()),
    ]
}

fn unix_milliseconds() -> Result<i64> {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| Error::internal("reading the clock for a Redis tag set", error))?;
    Ok(milliseconds(since))
}

#[cfg(feature = "memory")]
struct Subscriber {
    connection: Connection,
    pinged: bool,
}

#[cfg(feature = "memory")]
fn pushed_kind(reply: &Reply) -> Option<String> {
    let (Reply::Array(Some(items)) | Reply::Push(items)) = reply else {
        return None;
    };
    match items.first()? {
        Reply::Bulk(Some(kind)) => Some(String::from_utf8_lossy(kind).to_ascii_lowercase()),
        Reply::Status(kind) => Some(kind.to_ascii_lowercase()),
        _ => None,
    }
}

#[cfg(feature = "memory")]
impl Subscriber {
    /// An idle tick answers no payloads, so the listener can notice its cache was dropped.
    async fn receive(&mut self) -> Result<Vec<String>> {
        loop {
            let Ok(reply) = timeout(SUBSCRIBED_IDLE, self.connection.read()).await else {
                if self.pinged {
                    return Err(Error::Unavailable(
                        "Redis did not answer a ping on the subscription".to_owned(),
                    ));
                }
                self.connection.send(&[Command::new("PING")]).await?;
                self.pinged = true;
                return Ok(Vec::new());
            };
            let reply = reply?;
            self.pinged = false;
            if let Reply::Error(message) = &reply {
                return Err(Error::internal_message(format!(
                    "Redis refused the subscription: {message}"
                )));
            }
            if pushed_kind(&reply).as_deref() != Some("message") {
                continue;
            }
            let payload = reply
                .into_items()
                .and_then(|items| items.into_iter().nth(2));
            let payload = payload.and_then(Reply::into_bytes).ok_or_else(|| {
                Error::internal_message("a Redis message arrived without its payload")
            })?;
            return Ok(vec![String::from_utf8_lossy(&payload).into_owned()]);
        }
    }
}

#[cfg(feature = "memory")]
impl Notifications for Subscriber {
    fn next(&mut self) -> BoxFuture<'_, Result<Vec<String>>> {
        Box::pin(self.receive())
    }
}
