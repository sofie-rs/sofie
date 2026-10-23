mod signature;

use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use sofie_client::{Outgoing, Response, Vendor};
use sofie_core::{Error, Result, Timestamp};

use crate::{DownloadLink, Pending, Reader, StorageKey, Store, Writer};
use signature::{Canonical, Credentials, query_string, sha256_hex, uri_encode};

/// S3 takes parts of 5 MiB or more but the last; one part is held in memory at a time.
const PART_SIZE: usize = 8 * 1024 * 1024;
/// A first part grows from here instead of from empty by doubling.
const FIRST_BUFFER: usize = 256 * 1024;
const READ_CHUNK: u64 = 8 * 1024 * 1024;
/// Enough for a type sniff, so a download opened and dropped does not hold a whole chunk.
const FIRST_READ_CHUNK: u64 = 256 * 1024;
const EMPTY_BODY_HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// `endpoint` defaults to AWS's for the region; another S3-compatible service names its own,
/// usually with `path_style`. On AWS the credentials need `s3:ListBucket` besides the object
/// permissions: without it a missing file answers 403, read as a failure rather than as absent.
pub struct S3Settings {
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub path_style: bool,
}

impl S3Settings {
    /// What the adapter's `Outgoing` is built with; `private` allows a service inside the network.
    pub fn vendor(&self, private: bool) -> Result<Vendor> {
        Ok(Vendor {
            base_url: Some(self.base_url()?),
            private: Some(private),
            max_response_bytes: Some(READ_CHUNK),
            ..Vendor::default()
        })
    }

    fn base_url(&self) -> Result<String> {
        let place = self.place()?;
        Ok(format!("{}://{}", place.scheme, place.host))
    }

    fn place(&self) -> Result<Place> {
        let is_bucket = (3..=63).contains(&self.bucket.len())
            && self.bucket.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
            });
        if !is_bucket {
            return Err(Error::internal_message(format!(
                "the bucket `{}` is not a bucket name: 3 to 63 lowercase letters, digits, `.` and `-`",
                self.bucket
            )));
        }
        let endpoint = self
            .endpoint
            .clone()
            .unwrap_or_else(|| format!("https://s3.{}.amazonaws.com", self.region));
        let (scheme, host) = split_endpoint(&endpoint)?;
        Ok(if self.path_style {
            Place {
                scheme,
                host,
                prefix: format!("/{}", self.bucket),
            }
        } else {
            Place {
                scheme,
                host: format!("{}.{host}", self.bucket),
                prefix: String::new(),
            }
        })
    }
}

struct Place {
    scheme: &'static str,
    /// As sofie-client sends it in `Host`: lowercase, the port only when not the scheme's own.
    host: String,
    prefix: String,
}

fn split_endpoint(endpoint: &str) -> Result<(&'static str, String)> {
    let refused = |reason: &str| {
        Error::internal_message(format!("the S3 endpoint `{endpoint}` is refused: {reason}"))
    };
    let (scheme, rest) = endpoint
        .split_once("://")
        .ok_or_else(|| refused("it is not an absolute URL"))?;
    let (scheme, default_port) = match scheme {
        "https" => ("https", ":443"),
        "http" => ("http", ":80"),
        _ => return Err(refused("its scheme is neither https nor http")),
    };
    let authority = rest.strip_suffix('/').unwrap_or(rest).to_ascii_lowercase();
    if authority.is_empty() || authority.contains(['/', '@', '?', '#']) {
        return Err(refused("it must be a scheme and a host, without a path"));
    }
    let host = authority
        .strip_suffix(default_port)
        .map_or_else(|| authority.clone(), str::to_owned);
    Ok((scheme, host))
}

#[derive(Clone)]
pub struct S3 {
    shared: Arc<Shared>,
}

struct Shared {
    http: Outgoing,
    credentials: Credentials,
    place: Place,
}

impl S3 {
    /// `http` must be built with `settings.vendor(..)`.
    pub fn new(http: Outgoing, settings: S3Settings) -> Result<Self> {
        let place = settings.place()?;
        Ok(Self {
            shared: Arc::new(Shared {
                http,
                credentials: Credentials {
                    access_key_id: settings.access_key_id,
                    secret_access_key: settings.secret_access_key,
                    region: settings.region,
                },
                place,
            }),
        })
    }
}

#[derive(Clone, Copy)]
enum Verb {
    Get,
    Put,
    Post,
    Delete,
}

impl Verb {
    fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Put => "PUT",
            Self::Post => "POST",
            Self::Delete => "DELETE",
        }
    }
}

struct Call<'a> {
    verb: Verb,
    key: &'a StorageKey,
    query: Vec<(String, String)>,
    range: Option<String>,
    if_match: Option<String>,
    body: Bytes,
}

impl<'a> Call<'a> {
    fn new(verb: Verb, key: &'a StorageKey) -> Self {
        Self {
            verb,
            key,
            query: Vec::new(),
            range: None,
            if_match: None,
            body: Bytes::new(),
        }
    }

    fn query(mut self, name: &str, value: &str) -> Self {
        self.query.push((name.to_owned(), value.to_owned()));
        self
    }

    fn body(self, body: Bytes) -> Self {
        Self { body, ..self }
    }
}

impl Shared {
    fn path(&self, key: &StorageKey) -> String {
        uri_encode(&format!("{}/{key}", self.place.prefix), true)
    }

    async fn send(&self, call: Call<'_>) -> Result<Response> {
        let path = self.path(call.key);
        let payload_hash = payload_hash(&call.body).await?;
        let now = Timestamp::now();
        let mut headers = vec![
            ("host".to_owned(), self.place.host.clone()),
            ("x-amz-content-sha256".to_owned(), payload_hash.clone()),
            ("x-amz-date".to_owned(), signature::amz_date(now)),
        ];
        if let Some(range) = &call.range {
            headers.push(("range".to_owned(), range.clone()));
        }
        if let Some(etag) = &call.if_match {
            headers.push(("if-match".to_owned(), etag.clone()));
        }
        let query = query_string(&call.query);
        let authorization = self.credentials.authorization(
            Canonical {
                method: call.verb.as_str(),
                path: &path,
                query: call.query,
                headers: headers.clone(),
                payload_hash: &payload_hash,
            },
            now,
        );
        let target = if query.is_empty() {
            path
        } else {
            format!("{path}?{query}")
        };
        let mut request = match call.verb {
            Verb::Get => self.http.get(&target),
            Verb::Put => self.http.put(&target),
            Verb::Post => self.http.post(&target),
            Verb::Delete => self.http.delete(&target),
        };
        for (name, value) in headers.iter().skip(1) {
            request = request.header(name, value);
        }
        request = request.header("authorization", &authorization);
        if !call.body.is_empty() {
            request = request.body("application/octet-stream", call.body);
        }
        request.send().await
    }

    async fn expect(&self, call: Call<'_>, operation: &str) -> Result<Response> {
        let response = self.send(call).await?;
        if (200..300).contains(&response.status()) && !is_error_document(response.body()) {
            return Ok(response);
        }
        Err(refused(&response, operation))
    }
}

async fn payload_hash(body: &Bytes) -> Result<String> {
    if body.is_empty() {
        return Ok(EMPTY_BODY_HASH.to_owned());
    }
    // An 8 MiB part hashes for milliseconds, which would stall the async worker.
    let body = body.clone();
    tokio::task::spawn_blocking(move || sha256_hex(&body))
        .await
        .map_err(|error| Error::internal("hashing an S3 request body", error))
}

/// S3 may answer 200 to completing an upload and still have failed it, after whitespace it sends
/// to keep the connection open; the root element decides, not a substring.
fn is_error_document(body: &[u8]) -> bool {
    let mut body = body.trim_ascii_start();
    if body.starts_with(b"<?xml") {
        let Some(end) = body.windows(2).position(|pair| pair == b"?>") else {
            return false;
        };
        body = body[end + 2..].trim_ascii_start();
    }
    body.strip_prefix(b"<Error")
        .is_some_and(|rest| matches!(rest.first(), Some(b'>' | b' ' | b'\t' | b'\r' | b'\n')))
}

/// S3's error code names the problem; its message and resource may carry more than a log should.
fn refused(response: &Response, operation: &str) -> Error {
    let code = element(response.body(), "Code").unwrap_or_default();
    Error::internal_message(format!(
        "S3 refused to {operation}: {} {code}",
        response.status()
    ))
}

/// The element's text with XML's five entities read; an S3-compatible server may escape an id.
fn element(body: &[u8], name: &str) -> Option<String> {
    let text = std::str::from_utf8(body).ok()?;
    let start = text.find(&format!("<{name}>"))? + name.len() + 2;
    let length = text[start..].find(&format!("</{name}>"))?;
    Some(unescaped(&text[start..start + length]))
}

fn unescaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let entity = [
            ("&amp;", '&'),
            ("&lt;", '<'),
            ("&gt;", '>'),
            ("&quot;", '"'),
            ("&apos;", '\''),
        ]
        .into_iter()
        .find(|(name, _)| after.starts_with(name));
        let (character, length) =
            entity.map_or(('&', 1), |(name, character)| (character, name.len()));
        out.push(character);
        rest = &after[length..];
    }
    out.push_str(rest);
    out
}

impl Store for S3 {
    fn writer(&self, key: &StorageKey) -> Pending<'_, Box<dyn Writer>> {
        let writer = S3Writer {
            shared: Arc::clone(&self.shared),
            key: key.clone(),
            buffer: BytesMut::with_capacity(FIRST_BUFFER),
            upload: None,
            settled: false,
            broken: false,
        };
        Box::pin(std::future::ready(Ok(Box::new(writer) as Box<dyn Writer>)))
    }

    fn reader(&self, key: &StorageKey) -> Pending<'_, Option<Box<dyn Reader>>> {
        let shared = Arc::clone(&self.shared);
        let key = key.clone();
        Box::pin(self.shared.http.operation("get", async move {
            let mut reader = S3Reader {
                shared,
                key,
                next: 0,
                total: 0,
                etag: None,
                first: None,
            };
            Ok(reader
                .open()
                .await?
                .then(|| Box::new(reader) as Box<dyn Reader>))
        }))
    }

    fn delete(&self, key: &StorageKey) -> Pending<'_, ()> {
        let key = key.clone();
        Box::pin(self.shared.http.operation("delete", async move {
            self.shared
                .expect(Call::new(Verb::Delete, &key), "delete a file")
                .await
                .map(drop)
        }))
    }

    fn exists(&self, key: &StorageKey) -> Pending<'_, bool> {
        let key = key.clone();
        Box::pin(self.shared.http.operation("exists", async move {
            let mut call = Call::new(Verb::Get, &key);
            call.range = Some("bytes=0-0".to_owned());
            let response = self.shared.send(call).await?;
            match response.status() {
                200 | 206 | 416 => Ok(true),
                404 => Ok(false),
                _ => Err(refused(&response, "look for a file")),
            }
        }))
    }

    fn download_url(&self, link: &DownloadLink<'_>) -> Result<String> {
        let seconds = link.valid_for.as_secs();
        let (content_type, disposition) = link.content_type.served();
        let place = &self.shared.place;
        let path = self.shared.path(link.key);
        let query = self.shared.credentials.presigned_query(
            "GET",
            &path,
            &place.host,
            vec![
                ("response-content-type".to_owned(), content_type.to_owned()),
                (
                    "response-content-disposition".to_owned(),
                    crate::content_disposition(disposition, link.filename),
                ),
            ],
            link.now,
            seconds,
        );
        Ok(format!("{}://{}{path}?{query}", place.scheme, place.host))
    }
}

struct Upload {
    id: String,
    etags: Vec<String>,
}

struct S3Writer {
    shared: Arc<Shared>,
    key: StorageKey,
    buffer: BytesMut,
    upload: Option<Upload>,
    settled: bool,
    broken: bool,
}

impl S3Writer {
    async fn start_upload(&self) -> Result<Upload> {
        let call = Call::new(Verb::Post, &self.key).query("uploads", "");
        let response = self.shared.expect(call, "start an upload").await?;
        let id = element(response.body(), "UploadId")
            .ok_or_else(|| Error::internal_message("S3 started an upload without an id"))?;
        Ok(Upload {
            id,
            etags: Vec::new(),
        })
    }

    async fn send_part(&mut self, part: Bytes) -> Result<()> {
        let upload = match self.upload.take() {
            Some(upload) => upload,
            None => self.start_upload().await?,
        };
        let upload = self.upload.insert(upload);
        let number = (upload.etags.len() + 1).to_string();
        let call = Call::new(Verb::Put, &self.key)
            .query("partNumber", &number)
            .query("uploadId", &upload.id)
            .body(part);
        let response = self.shared.expect(call, "store a part").await?;
        let etag = response
            .header("etag")
            .filter(|etag| !etag.contains(['<', '&']))
            .ok_or_else(|| Error::internal_message("S3 stored a part without an ETag"))?;
        upload.etags.push(etag.to_owned());
        Ok(())
    }

    async fn complete(&mut self) -> Result<()> {
        let rest = self.buffer.split().freeze();
        let unsent = |upload: &Upload| !rest.is_empty() || upload.etags.is_empty();
        if self.upload.as_ref().is_some_and(unsent) {
            self.send_part(rest.clone()).await?;
        }
        let Some(upload) = &self.upload else {
            let call = Call::new(Verb::Put, &self.key).body(rest);
            return self.shared.expect(call, "store a file").await.map(drop);
        };
        let mut parts = String::from("<CompleteMultipartUpload>");
        for (index, etag) in upload.etags.iter().enumerate() {
            parts.push_str("<Part><PartNumber>");
            parts.push_str(&(index + 1).to_string());
            parts.push_str("</PartNumber><ETag>");
            parts.push_str(etag);
            parts.push_str("</ETag></Part>");
        }
        parts.push_str("</CompleteMultipartUpload>");
        let call = Call::new(Verb::Post, &self.key)
            .query("uploadId", &upload.id)
            .body(Bytes::from(parts));
        self.shared
            .expect(call, "complete an upload")
            .await
            .map(drop)
    }
}

impl Writer for S3Writer {
    fn write(&mut self, chunk: Bytes) -> Pending<'_, ()> {
        self.buffer.extend_from_slice(&chunk);
        Box::pin(async move {
            if self.broken {
                return Err(broken());
            }
            while self.buffer.len() >= PART_SIZE {
                let part = self.buffer.split_to(PART_SIZE).freeze();
                if let Err(error) = self.send_part(part).await {
                    self.broken = true;
                    return Err(error);
                }
            }
            Ok(())
        })
    }

    fn finish(mut self: Box<Self>) -> Pending<'static, ()> {
        let http = self.shared.http.clone();
        Box::pin(async move {
            if self.broken {
                return Err(broken());
            }
            let completing = async move {
                self.complete().await?;
                self.settled = true;
                Ok(())
            };
            http.operation("put", Box::pin(completing)).await
        })
    }

    fn abort(mut self: Box<Self>) -> Pending<'static, ()> {
        self.settled = true;
        let upload = self.upload.take();
        Box::pin(abort(Arc::clone(&self.shared), self.key.clone(), upload))
    }
}

fn broken() -> Error {
    Error::internal_message("an S3 upload lost a part to a failed write, so it cannot be finished")
}

async fn abort(shared: Arc<Shared>, key: StorageKey, upload: Option<Upload>) -> Result<()> {
    let Some(upload) = upload else {
        return Ok(());
    };
    let call = Call::new(Verb::Delete, &key).query("uploadId", &upload.id);
    shared.expect(call, "abort an upload").await.map(drop)
}

/// A writer dropped half way leaves no parts for S3 to keep and bill.
impl Drop for S3Writer {
    fn drop(&mut self) {
        let Some(upload) = self.upload.take().filter(|_| !self.settled) else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let aborting = abort(Arc::clone(&self.shared), self.key.clone(), Some(upload));
        runtime.spawn(async move {
            if let Err(error) = aborting.await {
                tracing::warn!(error.kind = "storage", error = %error, "aborting an S3 upload failed");
            }
        });
    }
}

struct S3Reader {
    shared: Arc<Shared>,
    key: StorageKey,
    next: u64,
    total: u64,
    /// Every later range must match it, so an overwrite meanwhile is an error, not a mix.
    etag: Option<String>,
    first: Option<Bytes>,
}

impl S3Reader {
    /// `false` when nothing is stored under the key.
    async fn open(&mut self) -> Result<bool> {
        let response = self.shared.send(self.range_call()).await?;
        match response.status() {
            404 => return Ok(false),
            416 => return Ok(true),
            200 => self.total = u64::try_from(response.body().len()).unwrap_or(u64::MAX),
            206 => self.total = self.checked_range(&response)?,
            _ => return Err(refused(&response, "read a file")),
        }
        self.etag = response.header("etag").map(str::to_owned);
        self.next = u64::try_from(response.body().len()).unwrap_or(u64::MAX);
        self.first = Some(response.into_body());
        Ok(true)
    }

    fn range_call(&self) -> Call<'_> {
        let length = if self.next == 0 {
            FIRST_READ_CHUNK
        } else {
            READ_CHUNK
        };
        let mut call = Call::new(Verb::Get, &self.key);
        call.range = Some(format!("bytes={}-{}", self.next, self.next + length - 1));
        call.if_match.clone_from(&self.etag);
        call
    }

    fn checked_range(&self, response: &Response) -> Result<u64> {
        let refused = |reason: &str| {
            Error::internal_message(format!(
                "S3 answered the range from byte {} {reason}",
                self.next
            ))
        };
        if response.status() != 206 {
            return Err(refused(&format!("with {}", response.status())));
        }
        let (start, total) = response
            .header("content-range")
            .and_then(|range| range.strip_prefix("bytes "))
            .and_then(|range| range.split_once('-'))
            .and_then(|(start, rest)| Some((start, rest.rsplit_once('/')?.1)))
            .and_then(|(start, total)| Some((start.parse::<u64>().ok()?, total.parse().ok()?)))
            .ok_or_else(|| refused("without a readable Content-Range"))?;
        if start != self.next {
            return Err(refused(&format!("with one from byte {start}")));
        }
        Ok(total)
    }
}

impl Reader for S3Reader {
    fn size(&self) -> Option<u64> {
        Some(self.total)
    }

    fn chunk(&mut self) -> Pending<'_, Option<Bytes>> {
        Box::pin(async move {
            if let Some(first) = self.first.take() {
                return Ok(Some(first).filter(|first| !first.is_empty()));
            }
            if self.next >= self.total {
                return Ok(None);
            }
            let response = self.shared.expect(self.range_call(), "read a file").await?;
            self.checked_range(&response)?;
            if response.body().is_empty() {
                return Err(Error::internal_message("S3 answered a range with no bytes"));
            }
            self.next += u64::try_from(response.body().len()).unwrap_or(u64::MAX);
            Ok(Some(response.into_body()))
        })
    }
}

#[cfg(test)]
mod tests;
