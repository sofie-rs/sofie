use std::sync::Arc;

use http::header::COOKIE;
use http::{HeaderMap, HeaderValue};
use sofie_auth::{SessionClaims, SigningKeys};
use sofie_core::{Error, Result, Timestamp};

use crate::Serving;

/// `__Host-` makes browsers refuse a cookie unless `Secure`, `Path=/` and without
/// `Domain`, so no other host can set or shadow it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CookieNames {
    session: Arc<str>,
    #[cfg(all(feature = "toolbar", debug_assertions))]
    toolbar: Arc<str>,
    #[cfg(feature = "flows")]
    flow: Arc<str>,
}

const SESSION_ATTRIBUTES: &str = "Path=/; Secure; HttpOnly; SameSite=Lax";
#[cfg(feature = "flows")]
const FLOW_SECONDS: i64 = 600;

impl CookieNames {
    /// The names `Serving::named` gave the app's cookies.
    pub fn of_serving(serving: &Serving) -> Result<Self> {
        Self::of(crate::AppName::of_serving(serving).as_str())
    }

    pub(crate) fn of(app: &str) -> Result<Self> {
        if app.is_empty() || !app.bytes().all(is_token_byte) {
            return Err(Error::internal_message(format!(
                "the app name `{app}` cannot name a cookie: it must be RFC 9110 token characters"
            )));
        }
        Ok(Self {
            session: format!("__Host-{app}-session").into(),
            #[cfg(all(feature = "toolbar", debug_assertions))]
            toolbar: format!("__Host-{app}-toolbar").into(),
            #[cfg(feature = "flows")]
            flow: format!("__Host-{app}-flow").into(),
        })
    }

    #[must_use]
    pub fn session(&self) -> &str {
        &self.session
    }

    #[cfg(all(feature = "toolbar", debug_assertions))]
    pub(crate) fn toolbar(&self) -> &str {
        &self.toolbar
    }

    pub(crate) fn session_value<'h>(&self, headers: &'h HeaderMap) -> Option<&'h str> {
        named(headers, &self.session)
    }

    /// The idle timeout is enforced when the cookie is opened.
    pub(crate) fn sealed_session(
        &self,
        keys: &SigningKeys,
        claims: &SessionClaims,
        now: Timestamp,
    ) -> Result<HeaderValue> {
        let sealed = claims.seal(keys)?;
        set(&self.session, &sealed, claims.remaining_seconds(now))
    }

    pub(crate) fn session_removal(&self) -> Result<HeaderValue> {
        set(&self.session, "", 0)
    }

    #[cfg(feature = "flows")]
    pub(crate) fn flow_value<'h>(&self, headers: &'h HeaderMap) -> Option<&'h str> {
        named(headers, &self.flow)
    }

    /// Lax, so the browser sends it back when a provider sends it to the app's callback.
    #[cfg(feature = "flows")]
    pub(crate) fn begun_flow(&self, value: &str) -> Result<HeaderValue> {
        set(&self.flow, value, FLOW_SECONDS)
    }
}

fn set(name: &str, value: &str, max_age: i64) -> Result<HeaderValue> {
    HeaderValue::try_from(format!(
        "{name}={value}; Max-Age={max_age}; {SESSION_ATTRIBUTES}"
    ))
    .map_err(|_| Error::internal_message(format!("the cookie `{name}` is not a header value")))
}

/// RFC 9110 `tchar`.
fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

/// Split as bytes: another cookie's non-ASCII value must not hide ours.
pub(crate) fn named<'h>(headers: &'h HeaderMap, name: &str) -> Option<&'h str> {
    headers
        .get_all(COOKIE)
        .iter()
        .flat_map(|header| header.as_bytes().split(|byte| *byte == b';'))
        .find_map(|pair| {
            let value = pair
                .trim_ascii()
                .strip_prefix(name.as_bytes())?
                .strip_prefix(b"=")?;
            std::str::from_utf8(value).ok()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_come_from_the_app() {
        let names = CookieNames::of("islands-demo").unwrap();

        assert_eq!(names.session(), "__Host-islands-demo-session");
        #[cfg(all(feature = "toolbar", debug_assertions))]
        assert_eq!(names.toolbar(), "__Host-islands-demo-toolbar");
    }

    #[test]
    fn a_non_ascii_cookie_beside_ours_does_not_hide_it() {
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_bytes("pref=Jürgen; __Host-shop-session=sealed".as_bytes()).unwrap(),
        );

        assert_eq!(named(&headers, "__Host-shop-session"), Some("sealed"));
        assert_eq!(named(&headers, "pref"), Some("Jürgen"));
    }

    #[test]
    fn a_name_that_cannot_name_a_cookie_is_refused() {
        for app in ["", "my shop", "shop;x", "shop=1", "şop"] {
            assert!(CookieNames::of(app).is_err(), "{app}");
        }
    }
}
