use std::fs::File;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Read;
use std::sync::OnceLock;

use bytes::Bytes;
use http::header::{ACCEPT, ACCEPT_LANGUAGE, AUTHORIZATION, CACHE_CONTROL, ETAG, VARY};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use sofie_core::{Error, Result, Uuid};
use sofie_db::Reply;

use crate::answer::{self, Response};
use crate::htmx;
use crate::pipeline::Asked;

/// Authenticated answers are kept by the browser only, and asked about again each time.
const REVALIDATE: &str = "private, no-cache";
const VARIES_BY: &str = "Accept, Accept-Language, Authorization, Cookie, HX-Request, HX-Target";
const KEYED_HEADERS: [HeaderName; 5] = [
    ACCEPT,
    ACCEPT_LANGUAGE,
    AUTHORIZATION,
    htmx::REQUEST,
    htmx::TARGET,
];
const CHUNK: usize = 64 * 1024;

/// A deploy may render the same rows differently, so the binary itself keys every tag.
static BUILD: OnceLock<u64> = OnceLock::new();

pub(crate) fn identify_build() -> Result<()> {
    if BUILD.get().is_some() {
        return Ok(());
    }
    let path = std::env::current_exe()
        .map_err(|error| Error::internal("finding the running executable", error))?;
    let mut file = File::open(&path)
        .map_err(|error| Error::internal(format!("opening {}", path.display()), error))?;
    let mut hasher = DefaultHasher::new();
    let mut chunk = vec![0; CHUNK];
    loop {
        let read = file
            .read(&mut chunk)
            .map_err(|error| Error::internal(format!("reading {}", path.display()), error))?;
        if read == 0 {
            break;
        }
        hasher.write(&chunk[..read]);
    }
    let _ = BUILD.set(hasher.finish());
    Ok(())
}

pub(crate) struct Keyed<'r> {
    pub(crate) asked: Asked<'r>,
    pub(crate) principal: Option<Uuid>,
    #[cfg(feature = "permissions")]
    pub(crate) permissions: &'r crate::Permissions,
    #[cfg(feature = "flags")]
    pub(crate) flags_version: Option<u64>,
    #[cfg(feature = "live")]
    pub(crate) seal_until: Option<i64>,
    /// A cache hit reads no rows, so its tag would not change with what it shows.
    #[cfg(feature = "screen-cache")]
    pub(crate) cached: bool,
    #[cfg(feature = "entitlements")]
    pub(crate) entitled: &'r [&'static str],
}

impl<'r> Keyed<'r> {
    pub(crate) fn of(
        asked: Asked<'r>,
        principal: Option<Uuid>,
        #[cfg_attr(
            not(any(feature = "live", feature = "screen-cache")),
            expect(unused_variables)
        )]
        target: &crate::target::Target,
        #[cfg(feature = "gates")]
        #[cfg_attr(
            not(any(feature = "permissions", feature = "flags", feature = "entitlements")),
            expect(unused_variables)
        )]
        gates: &'r crate::gate::RequestGates,
        #[cfg_attr(not(feature = "live"), expect(unused_variables))] now: sofie_core::Timestamp,
    ) -> Self {
        Self {
            asked,
            principal,
            #[cfg(feature = "permissions")]
            permissions: &gates.permissions,
            #[cfg(feature = "flags")]
            flags_version: gates.features.as_ref().map(sofie_flags::Features::version),
            #[cfg(feature = "live")]
            seal_until: (!target.admission.rules.live.is_empty())
                .then(|| crate::live::seal_until(now.as_second())),
            #[cfg(feature = "screen-cache")]
            cached: target.admission.rules.cached.is_some(),
            #[cfg(feature = "entitlements")]
            entitled: &gates.entitled,
        }
    }

    /// Flags change what a screen shows without a row read, so their version keys the tag;
    /// development templates change on disk, so nothing is tagged there.
    fn seed(&self) -> Option<u64> {
        let request = self.asked.request;
        if cfg!(feature = "development") || !matches!(request.method, Method::GET | Method::HEAD) {
            return None;
        }
        #[cfg(feature = "screen-cache")]
        if self.cached {
            return None;
        }
        let mut hasher = DefaultHasher::new();
        BUILD.get()?.hash(&mut hasher);
        self.principal.hash(&mut hasher);
        #[cfg(feature = "flags")]
        self.flags_version.hash(&mut hasher);
        #[cfg(feature = "permissions")]
        self.permissions.hash(&mut hasher);
        #[cfg(feature = "live")]
        self.seal_until.hash(&mut hasher);
        #[cfg(feature = "entitlements")]
        self.entitled.hash(&mut hasher);
        request
            .uri
            .path_and_query()
            .map(http::uri::PathAndQuery::as_str)
            .hash(&mut hasher);
        for name in &KEYED_HEADERS {
            for value in request.headers.get_all(name) {
                value.as_bytes().hash(&mut hasher);
            }
        }
        Some(hasher.finish())
    }

    pub(crate) async fn tagged(self, answer: impl Future<Output = Response>) -> Response {
        let Some(seed) = self.seed() else {
            return answer.await;
        };
        let (response, fingerprint) = sofie_db::track_reads(seed, answer).await;
        let Some(fingerprint) = fingerprint else {
            return response;
        };
        let tag = tag_of(fingerprint);
        let mut response = match response.status() {
            StatusCode::OK if is_held(&self.asked.request.headers, &tag) => not_modified(&tag),
            StatusCode::OK | StatusCode::NOT_MODIFIED => response,
            _ => return response,
        };
        let headers = response.headers_mut();
        headers.insert(ETAG, tag);
        headers.insert(CACHE_CONTROL, HeaderValue::from_static(REVALIDATE));
        headers.append(VARY, HeaderValue::from_static(VARIES_BY));
        response
    }
}

/// Before rendering: the rows read so far are all a loader's answer depends on.
pub(crate) fn unchanged(asked: Asked<'_>, loaded: &Result<Reply>) -> Option<Response> {
    let fingerprint = sofie_db::reads_so_far().filter(|_| loaded.is_ok())?;
    let tag = tag_of(fingerprint);
    is_held(&asked.request.headers, &tag).then(|| not_modified(&tag))
}

fn tag_of(fingerprint: u64) -> HeaderValue {
    let tag = format!("W/\"{fingerprint:016x}\"");
    HeaderValue::from_str(&tag).expect("hex digits in quotes make a valid header value")
}

fn is_held(request: &HeaderMap, tag: &HeaderValue) -> bool {
    tag.to_str().is_ok_and(|tag| answer::is_held(request, tag))
}

/// No `Content-Security-Policy`: the browser keeps the cached page's, whose nonce its body holds.
fn not_modified(tag: &HeaderValue) -> Response {
    let mut response = answer::response(StatusCode::NOT_MODIFIED, None, Bytes::new());
    response.headers_mut().insert(ETAG, tag.clone());
    response
}

#[cfg(test)]
mod tests {
    use http::header::IF_NONE_MATCH;

    use super::*;

    fn asking(if_none_match: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(IF_NONE_MATCH, HeaderValue::from_str(if_none_match).unwrap());
        headers
    }

    #[test]
    fn a_held_tag_matches_weakly_in_any_position() {
        let tag = tag_of(0xab);

        assert!(is_held(&asking("W/\"00000000000000ab\""), &tag));
        assert!(is_held(&asking("\"00000000000000ab\""), &tag));
        assert!(is_held(&asking("\"x\", W/\"00000000000000ab\""), &tag));
        assert!(is_held(&asking("*"), &tag));
        assert!(!is_held(&asking("W/\"00000000000000ac\""), &tag));
        assert!(!is_held(&HeaderMap::new(), &tag));
    }
}
