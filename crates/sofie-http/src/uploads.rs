use std::collections::HashSet;
use std::fmt::{self, Display};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use http::request::Parts;
use http::{Method, StatusCode};
use hyper::body::{Body, Incoming};
use serde::Deserialize;
use serde_json::{Map, Value};
use sofie_auth::SigningKeys;
use sofie_core::{Error, Result, unpoisoned};
use sofie_storage::{ContentType, Put, Storage, StorageKey};
use tokio::time::Instant;

use crate::Serving;
use crate::answer::{self, Response};
use crate::caller::Caller;
use crate::deadline::{Deadline, Paused};
use crate::multipart::{self, Multipart, PartHead, Stop};
use crate::pipeline::{Asked, RequestBody, discard_rest, read_body, too_large};
use crate::routes::BODY_LIMIT;
use crate::target::Target;
use crate::urlencoded::Encoding;

/// Text fields are held in memory, files are not, so text keeps the default limit however large
/// the route's; on a route taking files, so does a body that is not a form.
const TEXT_LIMIT: usize = BODY_LIMIT;
const PART_LIMIT: usize = 100;
const FILENAME_LIMIT: usize = 255;
const SEAL: &str = "seal";
/// A sealed upload is read back within its own request, which ends within this.
const SEAL_LIFETIME: Duration = Duration::from_mins(2);
/// While a file arrives the request deadline pauses; this still bounds the whole form.
const FORM_TIME: Duration = Duration::from_mins(10);

tokio::task_local! {
    /// What the handler's input took, as the input is read through serde, which carries nothing.
    static TAKEN: Arc<Mutex<Taken>>;
}

#[derive(Default)]
struct Taken {
    keys: HashSet<StorageKey>,
    committed: bool,
}

/// A record the commit wrote may name a taken upload, so whatever is answered after it, such as
/// a follow-up page that fails, keeps the file.
pub(crate) fn committed() {
    // Outside a request's handler, such as in a test, no stored file waits to be settled.
    let _outside_a_request = TAKEN.try_with(|taken| unpoisoned::lock(taken).committed = true);
}

/// A file received with the request, already in the app's [`Storage`]. Only the framework makes
/// one, from a `multipart/form-data` body; a JSON body cannot name a stored file.
#[derive(Clone, PartialEq, Eq)]
pub struct Upload {
    key: StorageKey,
    filename: String,
    content_type: ContentType,
    size: u64,
}

impl Upload {
    #[must_use]
    pub fn key(&self) -> &StorageKey {
        &self.key
    }

    /// The name it had on the sender's machine, without its folders or characters unsafe in a
    /// file name; never a path in storage.
    #[must_use]
    pub fn filename(&self) -> &str {
        &self.filename
    }

    /// As declared, and matching the file's first bytes for the types those tell apart.
    #[must_use]
    pub fn content_type(&self) -> &ContentType {
        &self.content_type
    }

    #[must_use]
    pub fn size(&self) -> u64 {
        self.size
    }

    fn sealed(&self, expires: u64) -> String {
        format!(
            "{}\n{}\n{}\n{}\n{expires}",
            self.key, self.content_type, self.size, self.filename
        )
    }

    fn to_json(&self) -> Result<Value> {
        self.sealed_json(seconds_now() + SEAL_LIFETIME.as_secs())
    }

    fn sealed_json(&self, expires: u64) -> Result<Value> {
        let sealed = self.sealed(expires);
        let seal = seal_keys()?.sign(sofie_auth::Signed::UploadSeal, sealed.as_bytes());
        Ok(serde_json::json!({
            "key": self.key,
            "filename": self.filename,
            "content_type": self.content_type,
            "size": self.size,
            "expires": expires,
            SEAL: seal,
        }))
    }
}

impl fmt::Debug for Upload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Upload")
            .field("key", &self.key)
            .field("content_type", &self.content_type)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Received {
    key: StorageKey,
    filename: String,
    content_type: ContentType,
    size: u64,
    expires: u64,
    seal: String,
}

impl<'de> Deserialize<'de> for Upload {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let not_received = || serde::de::Error::custom("is not a file sent with this form");
        let received = Received::deserialize(deserializer).map_err(|_| not_received())?;
        let upload = Self {
            key: received.key,
            filename: received.filename,
            content_type: received.content_type,
            size: received.size,
        };
        let keys = seal_keys().map_err(Error::into_deserialize_error)?;
        let sealed = upload.sealed(received.expires);
        if received.expires <= seconds_now()
            || !keys.verifies(
                sofie_auth::Signed::UploadSeal,
                sealed.as_bytes(),
                &received.seal,
            )
        {
            return Err(not_received());
        }
        // Outside a request's handler, such as in a test, no stored file waits to be settled.
        let _outside_a_request = TAKEN.try_with(|taken| {
            unpoisoned::lock(taken).keys.insert(upload.key.clone());
        });
        Ok(upload)
    }
}

#[cfg(feature = "engine")]
impl sofie_values::Present for Upload {
    const INPUT: sofie_values::Input = sofie_values::Input::new(sofie_values::Control::File);
    const SEARCH: sofie_values::Search = sofie_values::Search::Skipped;

    fn csv(&self) -> String {
        self.filename.clone()
    }
}

pub(crate) fn is_upload(value: &Value) -> bool {
    value.get(SEAL).is_some()
}

fn seconds_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// The process's own: an upload is sealed and opened within one request.
fn seal_keys() -> Result<&'static SigningKeys> {
    static KEYS: OnceLock<SigningKeys> = OnceLock::new();
    if let Some(keys) = KEYS.get() {
        return Ok(keys);
    }
    let generated = SigningKeys::generate()?;
    Ok(KEYS.get_or_init(|| generated))
}

/// The files stored while reading a request, deleted unless its handler commits with an input
/// that took them; a request cut by its timeout or a panic drops them unsettled, so they are
/// deleted then too.
#[derive(Default)]
pub(crate) struct Stored {
    storage: Option<Storage>,
    keys: Vec<StorageKey>,
    taken: Arc<Mutex<Taken>>,
}

impl Stored {
    pub(crate) async fn taking<F: Future>(&self, handler: F) -> F::Output {
        TAKEN.scope(Arc::clone(&self.taken), handler).await
    }

    pub(crate) async fn settle(mut self) {
        let mut keys = std::mem::take(&mut self.keys);
        {
            let taken = unpoisoned::lock(&self.taken);
            if taken.committed {
                keys.retain(|key| !taken.keys.contains(key));
            }
        }
        if let Some(storage) = &self.storage {
            discard(storage, &keys).await;
        }
    }
}

impl Drop for Stored {
    fn drop(&mut self) {
        let keys = std::mem::take(&mut self.keys);
        if let Some(storage) = self.storage.take()
            && !keys.is_empty()
        {
            tokio::spawn(async move { discard(&storage, &keys).await });
        }
    }
}

async fn discard(storage: &Storage, keys: &[StorageKey]) {
    for key in keys {
        if let Err(error) = storage.delete(key).await {
            tracing::warn!(error.kind = "storage", error = %error, "deleting an unused upload failed");
        }
    }
}

/// A `multipart/form-data` body sent to an action becomes the JSON object of its fields, each file
/// a sealed [`Upload`] stored as it arrives; any other body is read as before. A file sent to a
/// route without `.file_limit(…)`, to a page or queue route, or to an app without storage, is
/// refused: nothing would keep it. A webhook's body is always read as sent.
pub(crate) async fn read(
    asked: Asked<'_>,
    target: &Target,
    serving: &Serving,
    mut body: Incoming,
    limit: usize,
) -> Result<RequestBody, Box<Response>> {
    let (request, caller) = (asked.request, asked.caller);
    let boundary = multipart::boundary(&request.headers).filter(|_| !is_webhook(target));
    let Some(boundary) = boundary else {
        let limit = match target.admission.rules.file_limit {
            Some(_) => limit.min(TEXT_LIMIT),
            None => limit,
        };
        let body = read_body(&mut body, limit, caller).await?;
        return Ok(RequestBody::plain(body, &request.headers));
    };
    let refused = |error: Error| Box::new(answer::failure(error, caller));
    let boundary = match boundary {
        Ok(boundary) => boundary,
        Err(stop) => return Err(refused_unread(body, malformed(&stop), caller).await),
    };
    let storage = serving
        .storage
        .as_ref()
        .filter(|_| takes_files(request, target));
    let rules = &target.admission.rules;
    let file_limit = rules.file_limit.unwrap_or(limit);
    let limit = match (rules.body_limit, rules.file_limit) {
        (None, Some(file_limit)) => file_limit.saturating_add(TEXT_LIMIT),
        _ => limit,
    };
    let mut form = Form::new(body, &boundary, limit, storage, file_limit, asked.deadline);
    let read = form.read().await;
    match read {
        Ok(()) => {
            let encoded = |fields: &Map<String, Value>| {
                serde_json::to_vec(fields)
                    .map(Bytes::from)
                    .map_err(|error| refused(Error::internal("encoding a form's fields", error)))
            };
            Ok(RequestBody {
                body: encoded(&form.fields)?,
                #[cfg(feature = "idempotency")]
                fingerprint: encoded(&as_sent(&form.fields))?,
                encoding: Encoding::Json,
                stored: form.stored,
            })
        }
        Err(failure) => {
            if matches!(failure, Failure::TakesNoFiles)
                && serving.storage.is_none()
                && target.admission.rules.file_limit.is_some()
            {
                tracing::warn!(
                    error.kind = "storage",
                    "a form sent a file, but the app has no storage; see `Serving::storage`"
                );
            }
            form.stored.settle().await;
            Err(Box::new(failure.answer(form.multipart, caller).await))
        }
    }
}

/// Each upload by what was sent of it, without where it was stored.
#[cfg(feature = "idempotency")]
fn as_sent(fields: &Map<String, Value>) -> Map<String, Value> {
    let mut sent = fields.clone();
    for value in sent.values_mut() {
        if let Value::Object(upload) = value
            && upload.contains_key(SEAL)
        {
            upload.retain(|name, _| matches!(name.as_str(), "filename" | "content_type" | "size"));
        }
    }
    sent
}

/// A queued job reads its input in another process, which cannot open an upload's seal.
/// A webhook's signature covers the body exactly as sent, so a multipart one is never parsed.
fn is_webhook(target: &Target) -> bool {
    #[cfg(feature = "webhooks")]
    return target.admission.rules.verify.is_some();
    #[cfg(not(feature = "webhooks"))]
    {
        let _ = target;
        false
    }
}

fn takes_files(request: &Parts, target: &Target) -> bool {
    #[cfg(feature = "jobs")]
    if target.admission.rules.queues {
        return false;
    }
    target.admission.rules.file_limit.is_some()
        && request.method != Method::GET
        && request.method != Method::HEAD
}

async fn refused_unread(mut body: Incoming, error: Error, caller: Caller<'_>) -> Box<Response> {
    discard_rest(&mut body).await;
    Box::new(answer::failure(error, caller))
}

enum Failure {
    Stopped(Stop),
    Refused(Error),
    /// A file sent where none is taken, or to an app without storage.
    TakesNoFiles,
    TooSlow,
}

impl From<Stop> for Failure {
    fn from(stop: Stop) -> Self {
        Self::Stopped(stop)
    }
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self::Refused(error)
    }
}

impl Failure {
    /// The rest of the body is read first, as `read_body` does.
    async fn answer(self, multipart: Multipart<Incoming>, caller: Caller<'_>) -> Response {
        discard_rest(&mut multipart.into_body()).await;
        match self {
            Self::Stopped(Stop::TooLarge) => too_large(caller),
            Self::Stopped(stop) => answer::failure(malformed(&stop), caller),
            Self::Refused(error) => answer::failure(error, caller),
            Self::TakesNoFiles => answer::refusal(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "This address does not take files.",
                caller,
            ),
            Self::TooSlow => answer::refusal(
                StatusCode::REQUEST_TIMEOUT,
                "The form took too long to arrive.",
                caller,
            ),
        }
    }
}

fn malformed(stop: &Stop) -> Error {
    let reason = match stop {
        Stop::TooLarge => "it is too large",
        Stop::Malformed(reason) => reason,
        Stop::Unread(reason) => reason,
    };
    Error::BadRequest(format!("the form cannot be read: {reason}"))
}

struct Form<'s, B = Incoming> {
    multipart: Multipart<B>,
    storage: Option<&'s Storage>,
    file_limit: usize,
    fields: Map<String, Value>,
    stored: Stored,
    deadline: &'s Deadline,
    paused: Option<Paused<'s>>,
    ends: Instant,
}

impl<'s, B: Body<Data = Bytes> + Unpin> Form<'s, B>
where
    B::Error: Display,
{
    fn new(
        body: B,
        boundary: &str,
        limit: usize,
        storage: Option<&'s Storage>,
        file_limit: usize,
        deadline: &'s Deadline,
    ) -> Self {
        Self {
            multipart: Multipart::new(body, boundary, limit),
            storage,
            file_limit,
            fields: Map::new(),
            stored: Stored {
                storage: storage.cloned(),
                keys: Vec::new(),
                taken: Arc::default(),
            },
            deadline,
            paused: None,
            ends: Instant::now() + FORM_TIME,
        }
    }

    async fn read(&mut self) -> Result<(), Failure> {
        let read = self.parts().await;
        self.paused = None;
        read
    }

    async fn next_part(&mut self) -> Result<Option<PartHead>, Failure> {
        within(self.ends, self.multipart.next_part()).await
    }

    async fn next_chunk(&mut self) -> Result<Option<Bytes>, Failure> {
        within(self.ends, self.multipart.next_chunk()).await
    }

    async fn parts(&mut self) -> Result<(), Failure> {
        let mut parts = 0;
        let mut text = 0;
        while let Some(head) = self.next_part().await? {
            parts += 1;
            if parts > PART_LIMIT {
                return Err(Error::BadRequest(format!(
                    "the form has more than {PART_LIMIT} fields"
                ))
                .into());
            }
            match head.filename {
                Some(_) => self.file(head).await?,
                None => text += self.text(&head.name, TEXT_LIMIT - text).await?,
            }
        }
        Ok(())
    }

    async fn text(&mut self, name: &str, room: usize) -> Result<usize, Failure> {
        let mut value = Vec::new();
        while let Some(chunk) = self.next_chunk().await? {
            if value.len() + chunk.len() > room {
                return Err(Stop::TooLarge.into());
            }
            value.extend_from_slice(&chunk);
        }
        let length = value.len();
        let value = String::from_utf8(value)
            .map_err(|_| Error::BadRequest("the form is not UTF-8".to_owned()))?;
        if !value.is_empty() {
            crate::form::insert(&mut self.fields, name, value)?;
        }
        Ok(length)
    }

    async fn file(&mut self, head: PartHead) -> Result<(), Failure> {
        let name = head.name;
        if !sofie_core::is_field_name(&name) || self.fields.contains_key(&name) {
            return Err(Error::BadRequest(format!(
                "the form's file field `{name}` is not a plain field name or is sent twice"
            ))
            .into());
        }
        if self.storage.is_some() && self.paused.is_none() {
            self.paused = Some(self.deadline.paused());
        }
        let Some(first) = self.next_chunk().await? else {
            return Ok(());
        };
        let declared = head
            .content_type
            .as_deref()
            .map_or_else(|| Ok(ContentType::octet_stream()), ContentType::parse)
            .map_err(|_| Error::invalid(&name, "does not declare a media type"))?;
        let Some(storage) = self.storage else {
            return Err(Failure::TakesNoFiles);
        };
        let mut put = storage.put().await?;
        let written = self.write(&mut put, first, &declared, &name).await;
        match written {
            Ok(()) => {
                let size = put.size();
                let key = put.finish().await?;
                self.stored.keys.push(key.clone());
                let upload = Upload {
                    key,
                    filename: sanitized(head.filename.as_deref().unwrap_or_default()),
                    content_type: declared,
                    size,
                };
                self.fields.insert(name, upload.to_json()?);
                Ok(())
            }
            Err(failure) => {
                if let Err(error) = put.abort().await {
                    tracing::warn!(error.kind = "storage", error = %error, "discarding a refused upload failed");
                }
                Err(failure)
            }
        }
    }

    async fn write(
        &mut self,
        put: &mut Put,
        first: Bytes,
        declared: &ContentType,
        name: &str,
    ) -> Result<(), Failure> {
        let mut start = Vec::with_capacity(ContentType::SNIFFED_LENGTH);
        let mut chunk = Some(first);
        while let Some(data) = chunk {
            let size = put.size() + u64::try_from(data.len()).unwrap_or(u64::MAX);
            if size > u64::try_from(self.file_limit).unwrap_or(u64::MAX) {
                return Err(Error::invalid(
                    name,
                    format!("must be at most {} bytes", self.file_limit),
                )
                .into());
            }
            let missing = ContentType::SNIFFED_LENGTH.saturating_sub(start.len());
            start.extend_from_slice(&data[..missing.min(data.len())]);
            put.write(data).await?;
            chunk = self.next_chunk().await?;
        }
        if !declared.matches(&start) {
            return Err(
                Error::invalid(name, format!("is not the {declared} it claims to be")).into(),
            );
        }
        Ok(())
    }
}

async fn within<T>(
    ends: Instant,
    reading: impl Future<Output = Result<T, Stop>>,
) -> Result<T, Failure> {
    let read = tokio::time::timeout_at(ends, reading).await;
    read.map_err(|_| Failure::TooSlow)?.map_err(Failure::from)
}

/// The last segment of what the sender named it, without characters a file system or a header
/// would misread.
fn sanitized(filename: &str) -> String {
    let base = filename.rsplit(['/', '\\']).next().unwrap_or_default();
    let kept: String = base
        .chars()
        .filter(|character| {
            !sofie_core::misleads_display(*character) && !"\"<>:|?*".contains(*character)
        })
        .collect();
    let trimmed =
        kept.trim_matches(|character: char| character == '.' || character.is_whitespace());
    let mut end = trimmed.len().min(FILENAME_LIMIT);
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    match &trimmed[..end] {
        "" => "file".to_owned(),
        kept => kept.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use hyper::body::{Body, Frame};
    use tokio::sync::mpsc;
    use tokio::time::sleep;

    use super::*;

    const BOUNDARY: &str = "trickle";

    struct Trickle(mpsc::Receiver<Bytes>);

    impl Body for Trickle {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
            self.0
                .poll_recv(context)
                .map(|chunk| chunk.map(|bytes| Ok(Frame::data(bytes))))
        }
    }

    /// A part whose value arrives a byte every `pause`, `bytes` times.
    fn trickled(head: &'static str, bytes: usize, pause: Duration) -> Trickle {
        let (sender, receiver) = mpsc::channel(1);
        tokio::spawn(async move {
            let opening = format!("--{BOUNDARY}\r\n{head}\r\n\r\n");
            sender.send(Bytes::from(opening)).await.unwrap();
            for _ in 0..bytes {
                sleep(pause).await;
                let _closed = sender.send(Bytes::from_static(b"x")).await;
            }
            let closing = format!("\r\n--{BOUNDARY}--\r\n");
            let _closed = sender.send(Bytes::from(closing)).await;
        });
        Trickle(receiver)
    }

    async fn read_within_deadline(body: Trickle) -> Option<Result<(), Failure>> {
        let directory = std::env::temp_dir().join(format!(
            "sofie-http-trickle-{}",
            StorageKey::generate().unwrap()
        ));
        let keys = Arc::new(SigningKeys::generate().unwrap());
        let storage = Storage::new(sofie_storage::Local::new(&directory, keys).unwrap());
        let deadline = Deadline::default();
        let mut form = Form::new(body, BOUNDARY, 1024, Some(&storage), 1024, &deadline);
        let read = deadline.within(form.read()).await;
        drop(form);
        std::fs::remove_dir_all(&directory).unwrap();
        read
    }

    #[tokio::test(start_paused = true)]
    async fn only_a_file_part_pauses_the_deadline_and_then_for_ten_minutes_at_most() {
        let text = r#"Content-Disposition: form-data; name="title""#;
        let file = r#"Content-Disposition: form-data; name="photo"; filename="a.bin""#;
        let under_the_silence_timeout = Duration::from_secs(50);

        let slow_text = read_within_deadline(trickled(text, 2, under_the_silence_timeout)).await;
        let slow_file = read_within_deadline(trickled(file, 4, under_the_silence_timeout)).await;
        let endless_file =
            read_within_deadline(trickled(file, 15, under_the_silence_timeout)).await;

        assert!(slow_text.is_none());
        assert!(matches!(slow_file, Some(Ok(()))));
        assert!(matches!(endless_file, Some(Err(Failure::TooSlow))));
    }

    #[test]
    fn a_filename_keeps_only_its_last_safe_segment() {
        for (sent, kept) in [
            ("photo.png", "photo.png"),
            ("C:\\Users\\ada\\photo.png", "photo.png"),
            ("../../etc/passwd", "passwd"),
            ("..", "file"),
            ("", "file"),
            (" .hidden. ", "hidden"),
            ("a\"b<c>|?*\u{0}\n.txt", "abc.txt"),
            ("Jürgen's résumé.pdf", "Jürgen's résumé.pdf"),
        ] {
            assert_eq!(sanitized(sent), kept, "{sent:?}");
        }
        let long = "é".repeat(200);
        assert_eq!(sanitized(&long).len(), 254);
    }

    fn upload() -> Upload {
        Upload {
            key: StorageKey::generate().unwrap(),
            filename: "photo.png".to_owned(),
            content_type: ContentType::parse("image/png").unwrap(),
            size: 12,
        }
    }

    #[test]
    fn only_an_upload_this_process_sealed_is_read_back() {
        let upload = upload();
        let json = upload.to_json().unwrap();

        assert_eq!(
            serde_json::from_value::<Upload>(json.clone()).unwrap(),
            upload
        );
        for (field, forged) in [
            ("key", Value::from(StorageKey::generate().unwrap().as_str())),
            ("size", Value::from(13)),
            ("expires", Value::from(u64::MAX)),
            ("content_type", Value::from("text/html")),
            ("filename", Value::from("other.png")),
            (SEAL, Value::from("AAAA")),
        ] {
            let mut forged_json = json.clone();
            forged_json[field] = forged;
            assert!(
                serde_json::from_value::<Upload>(forged_json).is_err(),
                "{field}"
            );
        }
        assert!(serde_json::from_value::<Upload>(Value::from("photo.png")).is_err());
        let expired = upload.sealed_json(seconds_now() - 1).unwrap();
        assert!(serde_json::from_value::<Upload>(expired).is_err());
    }
}
