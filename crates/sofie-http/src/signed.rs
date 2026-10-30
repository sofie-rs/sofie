use sofie_db::{Context, Resolve};
use std::sync::Arc;
use std::time::Duration;

use http::StatusCode;
use sofie_auth::SigningKeys;
use sofie_core::{Clock, Error, Result, Timestamp};
use sofie_db::worker::JobContext;

use crate::admission::Visit;
use crate::answer::{self, Response};
use crate::caller::Caller;
use crate::guard::is_local_path;
use crate::{Area, RequestContext};

const EXPIRES: &str = "expires=";
const SIGNATURE: &str = "&signature=";
const UNSIGNED: &str = "This link is invalid or has expired.";

/// Opens a `.signed()` route without a session until it expires.
pub struct SignedUrls {
    keys: Arc<SigningKeys>,
    clock: Clock,
}

impl SignedUrls {
    /// Anything added to the link later breaks the signature.
    pub fn sign(&self, path: &str, valid_for: Duration) -> Result<String> {
        if !is_local_path(path) {
            return Err(Error::internal_message(format!(
                "only a path of this app can be signed, not `{path}`"
            )));
        }
        // A browser percent-encodes anything else and keeps a fragment, so the signature would fail.
        if !path
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'#')
        {
            return Err(Error::internal_message(format!(
                "a signed path must be percent-encoded first (`percent_encode`), not `{path}`"
            )));
        }
        let expires = self.clock.now().checked_add(valid_for)?.timestamp();
        let separator = if path.contains('?') { '&' } else { '?' };
        let signed = format!("{path}{separator}{EXPIRES}{}", expires.as_second());
        let signature = self.keys.sign(sofie_auth::Signed::Link, signed.as_bytes());
        Ok(format!("{signed}{SIGNATURE}{signature}"))
    }
}

/// Encrypts small values with the app's keys, each for one purpose: a value sealed for
/// one purpose never opens for another, and none opens without the keys.
pub struct Sealing {
    keys: Arc<SigningKeys>,
}

impl Sealing {
    pub fn seal(&self, purpose: &str, plain: &[u8]) -> Result<String> {
        self.keys.seal_value(purpose, plain)
    }

    #[must_use]
    pub fn open(&self, purpose: &str, sealed: &str) -> Option<Vec<u8>> {
        self.keys.open_value(purpose, sealed)
    }
}

impl<A: Area, M> Resolve<RequestContext<A, M>> for Sealing {
    fn resolve(context: &mut RequestContext<A, M>) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Ok(Self {
            keys: Arc::clone(&context.matched.keys),
        }))
    }
}

impl<A: Area, M> Resolve<RequestContext<A, M>> for SignedUrls {
    fn resolve(context: &mut RequestContext<A, M>) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Ok(Self {
            keys: Arc::clone(&context.matched.keys),
            clock: context.clock(),
        }))
    }
}

impl Resolve<JobContext> for SignedUrls {
    fn resolve(context: &mut JobContext) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(context.signing_keys().map(|keys| Self {
            keys,
            clock: context.clock(),
        }))
    }
}

pub(crate) fn is_signed_request(visit: Visit<'_>) -> bool {
    let uri = &visit.asked.request.uri;
    uri.path_and_query()
        .is_some_and(|path| is_signed(path.as_str(), visit.keys, visit.now))
}

pub(crate) fn unsigned(caller: Caller<'_>) -> Response {
    answer::refusal(StatusCode::FORBIDDEN, UNSIGNED, caller)
}

/// The verified `expires` and `signature` belong to the link, not to the query a handler reads.
pub(crate) fn unsigned_query(query: &str) -> &str {
    let Some((signed, _)) = query.rsplit_once(SIGNATURE) else {
        return query;
    };
    match signed.rsplit_once('&') {
        Some((asked, expires)) if expires.starts_with(EXPIRES) => asked,
        None if signed.starts_with(EXPIRES) => "",
        _ => query,
    }
}

/// The signature comes last, right after the expiry it covers.
fn is_signed(path_and_query: &str, keys: &SigningKeys, now: Timestamp) -> bool {
    let Some((signed, signature)) = path_and_query.rsplit_once(SIGNATURE) else {
        return false;
    };
    let expires = signed
        .rsplit(['?', '&'])
        .next()
        .and_then(|parameter| parameter.strip_prefix(EXPIRES))
        .and_then(|seconds| seconds.parse::<i64>().ok());
    expires.is_some_and(|expires| now.as_second() < expires)
        && keys.verifies(sofie_auth::Signed::Link, signed.as_bytes(), signature)
}

#[cfg(test)]
mod tests {
    use sofie_core::TestClock;

    use super::*;

    fn signer(clock: &TestClock) -> (SignedUrls, Arc<SigningKeys>) {
        let keys = Arc::new(SigningKeys::generate().unwrap());
        let urls = SignedUrls {
            keys: Arc::clone(&keys),
            clock: clock.clock(),
        };
        (urls, keys)
    }

    #[test]
    fn a_signed_query_reads_without_its_expiry_and_signature() {
        assert_eq!(
            unsigned_query("size=large&expires=9&signature=abc"),
            "size=large"
        );
        assert_eq!(unsigned_query("expires=9&signature=abc"), "");
        assert_eq!(unsigned_query("size=large"), "size=large");
    }

    #[test]
    fn a_signed_path_opens_until_it_expires() {
        let clock = TestClock::starting_at(Timestamp::from_second(1_700_000_000).unwrap());
        let (urls, keys) = signer(&clock);
        let minute = Duration::from_secs(60);

        let link = urls.sign("/files/7?size=large", minute).unwrap();

        assert!(link.starts_with("/files/7?size=large&expires=1700000060&signature="));
        assert!(is_signed(&link, &keys, clock.clock().now().timestamp()));
        clock.advance(minute).unwrap();
        assert!(!is_signed(&link, &keys, clock.clock().now().timestamp()));
    }

    #[test]
    fn a_changed_or_unsigned_path_does_not_open() {
        let clock = TestClock::starting_at(Timestamp::from_second(1_700_000_000).unwrap());
        let (urls, keys) = signer(&clock);
        let now = clock.clock().now().timestamp();
        let link = urls.sign("/files/7", Duration::from_secs(60)).unwrap();

        for forged in [
            link.replace("/files/7", "/files/8"),
            link.replace("expires=1700000060", "expires=1800000000"),
            format!("{link}&size=large"),
            "/files/7".to_owned(),
            "/files/7?expires=1800000000&signature=".to_owned(),
        ] {
            assert!(!is_signed(&forged, &keys, now), "{forged}");
        }
        let other_keys = SigningKeys::generate().unwrap();
        assert!(!is_signed(&link, &other_keys, now));
        assert!(
            urls.sign("https://evil.test/x", Duration::from_secs(1))
                .is_err()
        );
    }

    #[test]
    fn a_path_a_browser_would_not_send_as_signed_is_refused() {
        let clock = TestClock::starting_at(Timestamp::from_second(1_700_000_000).unwrap());
        let (urls, _) = signer(&clock);
        let minute = Duration::from_secs(60);

        assert!(urls.sign("/files/café", minute).is_err());
        assert!(urls.sign("/files/a b", minute).is_err());
        assert!(urls.sign("/files/7#page=2", minute).is_err());
        assert!(urls.sign("/files/caf%C3%A9", minute).is_ok());
    }
}
