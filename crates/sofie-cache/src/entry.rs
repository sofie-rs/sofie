use std::fmt::Display;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use sofie_core::{Error, Result};
use sofie_db::{Aggregate, Id};

use crate::cache::State;
use crate::flight::{Flight, Landed};
use crate::key::{aggregate_tag, any_record_tag, push_escaped_part, record_tag};
use crate::shared::Stored;

/// Redis keeps the sets naming a tag's entries this long, so no entry outlives its tags.
pub const MAX_EXPIRY: Duration = Duration::from_hours(7 * 24);
/// Redis and Postgres keep expiries in whole milliseconds.
const MIN_EXPIRY: Duration = Duration::from_millis(1);
/// Postgres refuses a primary key past about 2.7 KB; parts are often request data.
const MAX_KEY_LENGTH: usize = 512;

/// Reads and deletes; writing needs an expiry first ([`Entry::expires_in`]).
pub struct Entry<'c, T> {
    state: &'c State,
    key: String,
    /// How much of `key` is its module and name; only that is logged, as parts may name people.
    named: usize,
    value: PhantomData<fn() -> T>,
}

pub struct Expiring<'c, T> {
    entry: Entry<'c, T>,
    expires_in: Duration,
    tags: Vec<String>,
}

impl<'c, T: Serialize + DeserializeOwned> Entry<'c, T> {
    pub(crate) fn new(state: &'c State, key: String) -> Self {
        Self {
            state,
            named: key.len(),
            key,
            value: PhantomData,
        }
    }

    #[must_use]
    pub fn part(mut self, part: impl Display) -> Self {
        self.key.push(':');
        let _ =
            std::fmt::Write::write_fmt(&mut EscapingPart(&mut self.key), format_args!("{part}"));
        self
    }

    pub async fn get(&self) -> Result<Option<T>> {
        let Some(value) = self.state.lookup(self.checked_key()?).await? else {
            return Ok(None);
        };
        Ok(self.decoded(&value))
    }

    pub async fn delete(&self) -> Result<()> {
        self.state.delete(self.checked_key()?).await
    }

    fn named(&self) -> &str {
        &self.key[..self.named]
    }

    fn checked_key(&self) -> Result<&str> {
        if self.key.len() > MAX_KEY_LENGTH {
            return Err(Error::internal_message(format!(
                "cache entry `{}` has parts making its key {} bytes, past the {MAX_KEY_LENGTH} a \
                 store keeps; key it by an id rather than by text",
                self.named(),
                self.key.len()
            )));
        }
        Ok(&self.key)
    }

    #[must_use]
    pub fn expires_in(self, expires_in: Duration) -> Expiring<'c, T> {
        Expiring {
            entry: self,
            expires_in,
            tags: Vec::new(),
        }
    }

    /// A value an earlier build wrote in another shape is a miss, not a failure.
    fn decoded(&self, value: &str) -> Option<T> {
        match serde_json::from_str(value) {
            Ok(decoded) => {
                sofie_db::read_cached(value);
                Some(decoded)
            }
            Err(error) => {
                tracing::warn!(cache.key = self.named(), error = %error, "a cached value no longer reads as its type; computing it again");
                None
            }
        }
    }

    /// Fails nothing a cache need not answer: a store that cannot be reached is a miss.
    async fn cached(&self) -> Option<T> {
        let read = match self.checked_key() {
            Ok(key) => self.state.lookup(key).await,
            Err(error) => Err(error),
        };
        match read {
            Ok(value) => self.decoded(&value?),
            Err(error) => {
                tracing::warn!(cache.key = self.named(), error = %error, "reading the cache failed; computing the value");
                None
            }
        }
    }
}

enum Boarded {
    Lead { recomputed: bool },
    Handed(Arc<str>),
    Leave,
}

/// Escapes as the part is written, so a part costs no String of its own.
struct EscapingPart<'k>(&'k mut String);

impl std::fmt::Write for EscapingPart<'_> {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        push_escaped_part(self.0, text);
        Ok(())
    }
}

impl<T: Serialize + DeserializeOwned> Expiring<'_, T> {
    /// Dropped when any record of `A` is written.
    #[must_use]
    pub fn depends_on<A: Aggregate>(mut self) -> Self {
        self.tags.push(aggregate_tag(A::MODULE, A::NAME));
        self
    }

    /// Dropped when that record is written.
    #[must_use]
    pub fn depends_on_record<A: Aggregate>(mut self, id: Id<A>) -> Self {
        self.tags.push(record_tag(A::MODULE, A::NAME, id.uuid()));
        let any = any_record_tag(A::MODULE, A::NAME);
        if !self.tags.contains(&any) {
            self.tags.push(any);
        }
        self
    }

    pub async fn set(&self, value: &T) -> Result<()> {
        self.check_expiry()?;
        let stored = self.stored(value)?;
        let key = self.entry.checked_key()?;
        self.entry.state.write(key, stored, true).await
    }

    /// One caller per key computes at a time in this process; the others wait and read its value.
    pub async fn get_or_compute<F, Computing>(&self, compute: F) -> Result<T>
    where
        F: FnOnce() -> Computing,
        Computing: Future<Output = Result<T>>,
    {
        self.check_expiry()?;
        if let Some(value) = self.entry.cached().await {
            return Ok(value);
        }
        let flight = self.entry.state.flights.board(&self.entry.key).await;
        let leading = match self.boarded(&flight) {
            Boarded::Lead { recomputed } => Some((flight, recomputed)),
            Boarded::Handed(value) => {
                drop(flight);
                if let Some(value) = self.entry.decoded(&value) {
                    return Ok(value);
                }
                None
            }
            Boarded::Leave => {
                drop(flight);
                None
            }
        };
        if leading.is_some()
            && let Some(value) = self.entry.cached().await
        {
            return Ok(value);
        }
        self.computed(compute, leading).await
    }

    /// Without a handed value, waiters on a shared store that is down would each compute in
    /// turn; a waiter computes again for the rest once, then waiters leave rather than queue.
    fn boarded(&self, flight: &Flight<'_>) -> Boarded {
        let Some(landed) = flight.landed() else {
            return Boarded::Lead { recomputed: false };
        };
        let invalidations = &self.entry.state.invalidations;
        if !invalidations.touched_since(landed.epoch, &self.entry.key, &self.tags) {
            return Boarded::Handed(Arc::clone(&landed.value));
        }
        if landed.recomputed {
            return Boarded::Leave;
        }
        Boarded::Lead { recomputed: true }
    }

    async fn computed<F, Computing>(
        &self,
        compute: F,
        leading: Option<(Flight<'_>, bool)>,
    ) -> Result<T>
    where
        F: FnOnce() -> Computing,
        Computing: Future<Output = Result<T>>,
    {
        let epoch = self.entry.state.invalidations.epoch();
        let value = compute().await?;
        let stored = self.stored(&value)?;
        let _flight = leading.map(|(mut flight, recomputed)| {
            flight.land(Landed {
                epoch,
                value: Arc::clone(&stored.value),
                recomputed,
            });
            flight
        });
        if let Err(error) = self.keep(epoch, stored).await {
            tracing::warn!(cache.key = self.entry.named(), error = %error, "storing a computed value in the cache failed");
        }
        Ok(value)
    }

    /// A drop that ran while the value was on its way to the store may have landed before it,
    /// so the value is taken back out. A drop only another instance saw is not known here.
    async fn keep(&self, epoch: u64, stored: Stored) -> Result<()> {
        let state = self.entry.state;
        let key = self.entry.checked_key()?;
        if state.invalidations.touched_since(epoch, key, &self.tags) {
            tracing::debug!(
                cache.key = self.entry.named(),
                "not caching a value computed while what it depends on was dropped"
            );
            return Ok(());
        }
        let written = state.write(key, stored, false).await;
        if state.invalidations.touched_since(epoch, key, &self.tags) {
            tracing::debug!(
                cache.key = self.entry.named(),
                "taking back a value dropped while it was being cached"
            );
            state.delete(key).await?;
        }
        written
    }

    fn check_expiry(&self) -> Result<()> {
        if (MIN_EXPIRY..=MAX_EXPIRY).contains(&self.expires_in) {
            return Ok(());
        }
        Err(Error::internal_message(format!(
            "cache entry `{}` expires in {:?}, outside what a cache keeps ({MIN_EXPIRY:?} to {MAX_EXPIRY:?})",
            self.entry.named(),
            self.expires_in
        )))
    }

    fn stored(&self, value: &T) -> Result<Stored> {
        let value = serde_json::to_string(value).map_err(|error| {
            Error::internal(
                format!("encoding cache entry `{}`", self.entry.named()),
                error,
            )
        })?;
        Ok(Stored {
            value: Arc::from(value),
            tags: self.tags.clone(),
            expires_in: self.expires_in,
        })
    }
}
