//! One Redis connection pool an app configures once (`SOFIE_REDIS_URL`), shared by the
//! framework's features and reachable from handlers and jobs, without a `redis` crate.

mod connection;
#[cfg(any(test, feature = "testing"))]
pub mod fake;
mod resp;
#[cfg(test)]
mod tests;
mod url;

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use sofie_core::{Backoff, Error, Result, unpoisoned};
use sofie_db::{Context, Db, Resolve};
use tokio::sync::Semaphore;
use tokio::time::{Instant, timeout};

use connection::{Connection, Target, checked};
pub use resp::{Command, Reply};
pub use url::RedisUrl;

const MOST_CONNECTIONS: usize = 16;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
/// A subscription quiet this long is pinged, and dropped when the ping goes unanswered as long.
const SUBSCRIBED_IDLE: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct Redis {
    client: Arc<Client>,
}

struct Client {
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

impl Redis {
    /// Fails when Redis cannot be reached, so a misconfigured app fails at boot.
    pub async fn connect(url: RedisUrl) -> Result<Self> {
        Self::reaching(Target::new(url)).await
    }

    async fn reaching(target: Target) -> Result<Self> {
        let redis = Self {
            client: Arc::new(Client {
                target,
                idle: Mutex::default(),
                permits: Semaphore::new(MOST_CONNECTIONS),
                reconnect: Mutex::default(),
            }),
        };
        redis.call(&[Command::new("PING")]).await?;
        Ok(redis)
    }

    /// Handlers and jobs on `db`, or on a clone of it, then take `Redis` as an argument.
    pub fn install(&self, db: &Db) {
        sofie_db::provide(db, Arc::new(self.clone()));
    }

    pub async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let reply = self.call(&[Command::new("GET").argument(key)]).await?;
        match reply.into_iter().next() {
            Some(Reply::Bulk(found)) => Ok(found),
            other => Err(Error::internal_message(format!(
                "Redis answered GET with {other:?}"
            ))),
        }
    }

    /// Every value expires, so nothing written here outlives its use unnoticed.
    pub async fn set(
        &self,
        key: &str,
        value: impl Into<Vec<u8>>,
        expires_in: Duration,
    ) -> Result<()> {
        let milliseconds = expires_in.as_millis().max(1).to_string();
        let set = Command::new("SET")
            .argument(key)
            .argument(value)
            .argument("PX")
            .argument(milliseconds);
        self.call(&[set]).await.map(drop)
    }

    pub async fn delete(&self, keys: &[&str]) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        let delete = keys
            .iter()
            .fold(Command::new("DEL"), |delete, key| delete.argument(*key));
        self.call(&[delete]).await.map(drop)
    }

    /// Pipelined on one connection; an idle connection that fails is replaced once, as Redis
    /// may have closed it meanwhile. An error reply fails the call.
    pub async fn call(&self, commands: &[Command]) -> Result<Vec<Reply>> {
        let client = &self.client;
        let _permit = timeout(COMMAND_TIMEOUT, client.permits.acquire())
            .await
            .map_err(|_| {
                Error::Unavailable(format!(
                    "all {MOST_CONNECTIONS} Redis connections stayed busy for {} ms",
                    COMMAND_TIMEOUT.as_millis()
                ))
            })?
            .map_err(|error| Error::internal("waiting for a Redis connection", error))?;
        let reused = client.idle().pop();
        if let Some(connection) = reused
            && let Ok(replies) = client.exchange(connection, commands).await
        {
            return checked(commands, replies);
        }
        let connection = client.open().await?;
        let replies = client.exchange(connection, commands).await?;
        checked(commands, replies)
    }

    /// Redis runs the commands together, with no other client's command between them.
    pub async fn transaction(&self, commands: Vec<Command>) -> Result<Vec<Reply>> {
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

    /// Pub/sub ignores the database number, so the channel names it: apps kept apart by database
    /// do not hear each other.
    fn channel(&self, channel: &str) -> String {
        format!("{channel}:{}", self.client.target.url.database)
    }

    pub async fn publish(&self, channel: &str, payload: &str) -> Result<()> {
        let publish = Command::new("PUBLISH")
            .argument(self.channel(channel))
            .argument(payload);
        self.call(&[publish]).await.map(drop)
    }

    /// On a connection of its own, as a subscribed connection takes no other commands.
    pub async fn subscribe(&self, channel: &str) -> Result<Subscription> {
        let opening = timeout(CONNECT_TIMEOUT, Connection::open(&self.client.target));
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
        Ok(Subscription {
            connection,
            pinged: false,
        })
    }

    #[must_use]
    pub fn installed(db: &Db) -> Option<Self> {
        sofie_db::provided::<Self>(db).map(|redis| (*redis).clone())
    }

    fn of(db: &Db) -> Result<Self> {
        Self::installed(db).ok_or_else(|| {
            Error::internal_message(
                "no Redis is connected; enable sofie's `redis` feature and set SOFIE_REDIS_URL",
            )
        })
    }
}

impl<C: Context> Resolve<C> for Redis {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Self::of(context.db()))
    }
}

impl Client {
    fn idle(&self) -> MutexGuard<'_, Vec<Connection>> {
        unpoisoned::lock(&self.idle)
    }

    fn reconnect(&self) -> MutexGuard<'_, Reconnect> {
        unpoisoned::lock(&self.reconnect)
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
}

pub struct Subscription {
    connection: Connection,
    pinged: bool,
}

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

impl Subscription {
    /// The next payloads published; none after a quiet spell, so a listener can notice it
    /// is no longer wanted.
    pub async fn next(&mut self) -> Result<Vec<String>> {
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
