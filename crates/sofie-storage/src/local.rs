use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use sofie_auth::SigningKeys;
use sofie_core::encoding::percent;
use sofie_core::{Error, Result, Timestamp};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{
    ContentType, DownloadLink, Pending, Reader, SERVED_PATH, Served, StorageKey, Store, Writer,
};

const CHUNK: usize = 64 * 1024;
const EXPIRES: &str = "?expires=";
const SIGNATURE: &str = "&signature=";
const NAME: &str = "&name=";
const PARTIAL: &str = "partial";
/// Longer than any upload takes, so a partial file this old was left by a killed process.
const ABANDONED_AFTER: Duration = Duration::from_hours(24);

pub struct Local {
    root: Arc<Path>,
    keys: Arc<SigningKeys>,
}

impl Local {
    /// `keys` must be the ones sofie-http checks requests with, so it accepts the links signed here.
    pub fn new(root: impl Into<PathBuf>, keys: Arc<SigningKeys>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|error| {
            Error::internal(
                format!("creating the storage root {}", root.display()),
                error,
            )
        })?;
        let swept = root.clone();
        std::thread::spawn(move || remove_abandoned(&swept));
        Ok(Self {
            root: root.into(),
            keys,
        })
    }

    fn path(&self, key: &StorageKey) -> PathBuf {
        self.root.join(key.as_str())
    }
}

impl Store for Local {
    fn writer(&self, key: &StorageKey) -> Pending<'_, Box<dyn Writer>> {
        let path = self.path(key);
        Box::pin(async move {
            // Its own name: a variant's key is derived, and a killed writer's partial file stays.
            let partial = path.with_extension(format!("{}.{PARTIAL}", StorageKey::generate()?));
            let file = File::options()
                .write(true)
                .create_new(true)
                .open(&partial)
                .await
                .map_err(|error| failed("creating", &partial, error))?;
            Ok(Box::new(LocalWriter {
                file: Some(file),
                partial,
                path,
                settled: false,
            }) as Box<dyn Writer>)
        })
    }

    fn reader(&self, key: &StorageKey) -> Pending<'_, Option<Box<dyn Reader>>> {
        let path = self.path(key);
        Box::pin(async move {
            match File::open(&path).await {
                Ok(file) => {
                    let size = file
                        .metadata()
                        .await
                        .map_err(|error| failed("reading the size of", &path, error))?
                        .len();
                    Ok(Some(Box::new(LocalReader { file, size }) as Box<dyn Reader>))
                }
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
                Err(error) => Err(failed("opening", &path, error)),
            }
        })
    }

    fn delete(&self, key: &StorageKey) -> Pending<'_, ()> {
        let path = self.path(key);
        Box::pin(async move {
            match tokio::fs::remove_file(&path).await {
                Err(error) if error.kind() != ErrorKind::NotFound => {
                    Err(failed("deleting", &path, error))
                }
                _ => Ok(()),
            }
        })
    }

    fn exists(&self, key: &StorageKey) -> Pending<'_, bool> {
        let path = self.path(key);
        Box::pin(async move {
            tokio::fs::try_exists(&path)
                .await
                .map_err(|error| failed("looking for", &path, error))
        })
    }

    fn download_url(&self, link: &DownloadLink<'_>) -> Result<String> {
        let expires = link
            .now
            .as_second()
            .saturating_add(i64::try_from(link.valid_for.as_secs()).unwrap_or(i64::MAX));
        let mut signed = format!(
            "{SERVED_PATH}{}/{}{EXPIRES}{expires}",
            link.key, link.content_type
        );
        if let Some(filename) = link.filename {
            signed.push_str(NAME);
            percent::push_encoded(&mut signed, filename, percent::is_unreserved);
        }
        let signature = self
            .keys
            .sign(sofie_auth::Signed::StoredFile, signed.as_bytes());
        Ok(format!("{signed}{SIGNATURE}{signature}"))
    }

    fn served(&self, path_and_query: &str, now: Timestamp) -> Option<Served> {
        let (signed, signature) = path_and_query.split_once(SIGNATURE)?;
        let (path, query) = signed.split_once(EXPIRES)?;
        let (expires, filename) = match query.split_once(NAME) {
            Some((expires, name)) => (expires, Some(query_value(name)?)),
            None => (query, None),
        };
        let expires: i64 = expires.parse().ok()?;
        let verified =
            self.keys
                .verifies(sofie_auth::Signed::StoredFile, signed.as_bytes(), signature);
        if now.as_second() >= expires || !verified {
            return None;
        }
        let (key, content_type) = path.strip_prefix(SERVED_PATH)?.split_once('/')?;
        Some(Served {
            key: StorageKey::parse(key).ok()?,
            content_type: ContentType::parse(content_type).ok()?,
            filename,
            fresh_for: Duration::from_secs(u64::try_from(expires - now.as_second()).ok()?),
        })
    }
}

fn remove_abandoned(root: &Path) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(error.kind = "storage", error = %error, "listing the storage root for abandoned partial files failed");
            return;
        }
    };
    for entry in entries {
        if let Err(error) = entry.and_then(|entry| remove_if_abandoned(&entry.path())) {
            tracing::warn!(error.kind = "storage", error = %error, "removing an abandoned partial file failed");
        }
    }
}

fn remove_if_abandoned(path: &Path) -> std::io::Result<()> {
    if path
        .extension()
        .is_none_or(|extension| extension != PARTIAL)
    {
        return Ok(());
    }
    let age = std::fs::metadata(path)?
        .modified()?
        .elapsed()
        .unwrap_or_default();
    if age < ABANDONED_AFTER {
        return Ok(());
    }
    std::fs::remove_file(path)
}

/// The filename `download_url` wrote; anything else is refused.
fn query_value(encoded: &str) -> Option<String> {
    String::from_utf8(percent::decode(encoded.as_bytes())?).ok()
}

/// Written beside its final name and renamed once complete, so a reader never sees half a file.
struct LocalWriter {
    file: Option<File>,
    partial: PathBuf,
    path: PathBuf,
    settled: bool,
}

impl LocalWriter {
    fn file(&mut self) -> &mut File {
        self.file
            .as_mut()
            .expect("only finish takes the file, and it consumes the writer")
    }
}

impl Writer for LocalWriter {
    fn write(&mut self, chunk: Bytes) -> Pending<'_, ()> {
        Box::pin(async move {
            self.file()
                .write_all(&chunk)
                .await
                .map_err(|error| failed("writing", &self.partial, error))
        })
    }

    fn finish(mut self: Box<Self>) -> Pending<'static, ()> {
        Box::pin(async move {
            self.file()
                .sync_all()
                .await
                .map_err(|error| failed("syncing", &self.partial, error))?;
            drop(self.file.take());
            tokio::fs::rename(&self.partial, &self.path)
                .await
                .map_err(|error| failed("renaming", &self.partial, error))?;
            self.settled = true;
            Ok(())
        })
    }

    fn abort(mut self: Box<Self>) -> Pending<'static, ()> {
        Box::pin(async move {
            self.settled = true;
            drop(self.file.take());
            tokio::fs::remove_file(&self.partial)
                .await
                .map_err(|error| failed("removing", &self.partial, error))
        })
    }
}

/// A writer dropped before it settled, by a failed request or finish, leaves no partial file.
impl Drop for LocalWriter {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        drop(self.file.take());
        if let Err(error) = std::fs::remove_file(&self.partial) {
            tracing::warn!(
                error.kind = "storage",
                error = %error,
                "removing an unfinished file failed"
            );
        }
    }
}

struct LocalReader {
    file: File,
    size: u64,
}

impl Reader for LocalReader {
    fn chunk(&mut self) -> Pending<'_, Option<Bytes>> {
        Box::pin(async move {
            let mut buffer = BytesMut::with_capacity(CHUNK);
            let read = self
                .file
                .read_buf(&mut buffer)
                .await
                .map_err(|error| Error::internal("reading a stored file", error))?;
            Ok((read > 0).then(|| buffer.freeze()))
        })
    }

    fn size(&self) -> Option<u64> {
        Some(self.size)
    }
}

fn failed(doing: &str, path: &Path, error: std::io::Error) -> Error {
    Error::internal(format!("{doing} {}", path.display()), error)
}
