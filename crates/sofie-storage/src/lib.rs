mod content_type;
mod disposition;
mod key;
#[cfg(feature = "local")]
mod local;
#[cfg(feature = "s3")]
mod s3;
#[cfg(feature = "variants")]
mod variants;

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

pub use bytes::Bytes;
use sofie_core::{Clock, Result, Timestamp};

pub use content_type::{ContentType, Disposition};
pub use disposition::content_disposition;
pub use key::StorageKey;
#[cfg(feature = "local")]
pub use local::Local;
#[cfg(feature = "s3")]
pub use s3::{S3, S3Settings};
#[cfg(feature = "variants")]
pub use variants::{MAX_ORIGINAL_BYTES, MadeVariant, Resized, Resizer, Variant};

/// Where sofie-http serves the links a store signs for the app itself to answer, before routing,
/// so no app route can take it.
pub const SERVED_PATH: &str = "/_sofie/files/";
/// The most S3 signs for, held for every store so a caller's mistake makes no lasting link.
const MAXIMUM_LINK_SECONDS: u64 = 7 * 24 * 60 * 60;

pub type Pending<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

pub trait Store: Send + Sync + 'static {
    fn writer(&self, key: &StorageKey) -> Pending<'_, Box<dyn Writer>>;

    fn reader(&self, key: &StorageKey) -> Pending<'_, Option<Box<dyn Reader>>>;

    /// Deleting a key that holds nothing succeeds.
    fn delete(&self, key: &StorageKey) -> Pending<'_, ()>;

    fn exists(&self, key: &StorageKey) -> Pending<'_, bool>;

    fn download_url(&self, link: &DownloadLink<'_>) -> Result<String>;

    /// The file a link this store signed for the app itself to serve names, while it is valid.
    fn served(&self, _path_and_query: &str, _now: Timestamp) -> Option<Served> {
        None
    }
}

/// Nothing is kept under its key until `finish` succeeds.
pub trait Writer: Send {
    fn write(&mut self, chunk: Bytes) -> Pending<'_, ()>;

    fn finish(self: Box<Self>) -> Pending<'static, ()>;

    fn abort(self: Box<Self>) -> Pending<'static, ()>;
}

pub trait Reader: Send {
    /// `None` once the file is read.
    fn chunk(&mut self) -> Pending<'_, Option<Bytes>>;

    /// In bytes, when the store knows it before the content is read.
    fn size(&self) -> Option<u64> {
        None
    }
}

pub struct DownloadLink<'a> {
    pub key: &'a StorageKey,
    pub content_type: &'a ContentType,
    /// What the browser saves it as; without one it names the file from the link.
    pub filename: Option<&'a str>,
    pub now: Timestamp,
    /// From a second to seven days: `Storage` refuses any other.
    pub valid_for: Duration,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Served {
    pub key: StorageKey,
    pub content_type: ContentType,
    pub filename: Option<String>,
    /// Until the link expires: the content under a key never changes, so a browser may keep it.
    pub fresh_for: Duration,
}

#[derive(Clone)]
pub struct Storage {
    store: Arc<dyn Store>,
    clock: Clock,
}

impl Storage {
    pub fn new(store: impl Store) -> Self {
        Self {
            store: Arc::new(store),
            clock: Clock::system(),
        }
    }

    #[must_use]
    pub fn with_clock(self, clock: Clock) -> Self {
        Self { clock, ..self }
    }

    pub async fn put(&self) -> Result<Put> {
        let key = StorageKey::generate()?;
        let writer = self.store.writer(&key).await?;
        Ok(Put {
            key,
            writer,
            size: 0,
        })
    }

    pub async fn get(&self, key: &StorageKey) -> Result<Option<Content>> {
        let reader = self.store.reader(key).await?;
        Ok(reader.map(|reader| Content { reader }))
    }

    pub async fn delete(&self, key: &StorageKey) -> Result<()> {
        self.store.delete(key).await
    }

    pub async fn exists(&self, key: &StorageKey) -> Result<bool> {
        self.store.exists(key).await
    }

    /// Opens the file to whoever holds the link until it expires; served as `content_type` only
    /// when that cannot run as a page.
    pub fn download_url(
        &self,
        key: &StorageKey,
        content_type: &ContentType,
        valid_for: Duration,
    ) -> Result<String> {
        self.store.download_url(&DownloadLink {
            key,
            content_type,
            filename: None,
            now: self.clock.now().timestamp(),
            valid_for: link_lifetime(valid_for)?,
        })
    }

    /// As `download_url`, saved by the browser as `filename`, such as an upload's own name.
    pub fn named_download_url(
        &self,
        key: &StorageKey,
        content_type: &ContentType,
        filename: &str,
        valid_for: Duration,
    ) -> Result<String> {
        self.store.download_url(&DownloadLink {
            key,
            content_type,
            filename: Some(filename),
            now: self.clock.now().timestamp(),
            valid_for: link_lifetime(valid_for)?,
        })
    }

    #[doc(hidden)]
    #[must_use]
    pub fn served(&self, path_and_query: &str) -> Option<Served> {
        self.store
            .served(path_and_query, self.clock.now().timestamp())
    }
}

pub struct Put {
    key: StorageKey,
    writer: Box<dyn Writer>,
    size: u64,
}

impl Put {
    #[must_use]
    pub fn key(&self) -> &StorageKey {
        &self.key
    }

    #[must_use]
    pub fn size(&self) -> u64 {
        self.size
    }

    pub async fn write(&mut self, chunk: Bytes) -> Result<()> {
        self.size += u64::try_from(chunk.len()).unwrap_or(u64::MAX);
        self.writer.write(chunk).await
    }

    pub async fn finish(self) -> Result<StorageKey> {
        self.writer.finish().await?;
        Ok(self.key)
    }

    pub async fn abort(self) -> Result<()> {
        self.writer.abort().await
    }
}

pub struct Content {
    reader: Box<dyn Reader>,
}

impl Content {
    pub async fn chunk(&mut self) -> Result<Option<Bytes>> {
        self.reader.chunk().await
    }

    #[must_use]
    pub fn size(&self) -> Option<u64> {
        self.reader.size()
    }
}

fn link_lifetime(valid_for: Duration) -> Result<Duration> {
    let seconds = valid_for.as_secs();
    if !(1..=MAXIMUM_LINK_SECONDS).contains(&seconds) {
        return Err(sofie_core::Error::internal_message(format!(
            "a download link lasts from a second to seven days, not {seconds} seconds"
        )));
    }
    Ok(valid_for)
}
