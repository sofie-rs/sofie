use std::sync::Arc;
use std::time::Duration;

use sofie_core::{Error, Result};
use sofie_db::BoxFuture;
use sofie_redis::{Command, Redis, Reply};

use crate::MAX_EXPIRY;
use crate::shared::{SharedStore, Stored, milliseconds};

/// Sorted sets of the keys under a tag, each scored by when its entry expires.
const TAG_PREFIX: &str = "sofie:tags:";
/// A member is pruned this long after its entry expires, so instances' clocks may disagree by it.
const CLOCK_SKEW: Duration = Duration::from_mins(1);
/// Members popped per tag per round trip, so no drop moves one huge reply.
const DROP_CHUNK: usize = 512;

pub(crate) struct RedisStore(pub(crate) Redis);

#[cfg(feature = "memory")]
pub(crate) struct Subscribed(pub(crate) sofie_redis::Subscription);

#[cfg(feature = "memory")]
impl sofie_db::Notifications for Subscribed {
    fn next(&mut self) -> BoxFuture<'_, Result<Vec<String>>> {
        Box::pin(self.0.next())
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

impl SharedStore for RedisStore {
    fn name(&self) -> &'static str {
        "redis"
    }

    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Option<Stored>>> {
        Box::pin(async move {
            let commands = [
                Command::new("GET").argument(key),
                Command::new("PTTL").argument(key),
            ];
            let mut replies = self.0.call(&commands).await?.into_iter();
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
            self.0.transaction(commands).await.map(drop)
        })
    }

    fn delete<'a>(&'a self, keys: &'a [String]) -> BoxFuture<'a, Result<()>> {
        let delete = keys.iter().fold(Command::new("DEL"), |delete, key| {
            delete.argument(key.as_str())
        });
        Box::pin(async move { self.0.call(&[delete]).await.map(drop) })
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

impl RedisStore {
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
        let read = self.0.call(&reads).await?;
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
        self.0.transaction(commands).await?;
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
