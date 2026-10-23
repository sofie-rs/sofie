use std::sync::{Arc, OnceLock};

use http::{HeaderMap, HeaderValue};
use sofie_auth::{SessionClaims, Signed, SigningKeys};
use sofie_core::{Error, Result, Timestamp, Uuid};

use crate::cookie;

/// No `Max-Age`: the overrides end with the browser session.
const ATTRIBUTES: &str = "Path=/; Secure; HttpOnly; SameSite=Strict";
const SEAL: &str = "&seal=";

/// What one browser's toolbar changed; the stored flags and the app's clock stay as they are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Overrides {
    pub(crate) acting_as: Option<ActingAs>,
    pub(crate) flags: Arc<[(String, String)]>,
    pub(crate) clock_seconds: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActingAs {
    pub(crate) area: String,
    pub(crate) principal: Uuid,
}

impl Overrides {
    pub(crate) fn of(headers: &HeaderMap, cookie_name: &str) -> Self {
        let Some(value) = cookie::named(headers, cookie_name) else {
            return Self::default();
        };
        let Some(value) = unsealed(value) else {
            tracing::debug!("ignored a toolbar cookie this process did not seal");
            return Self::default();
        };
        Self::parse(value).unwrap_or_else(|error| {
            tracing::warn!(error = %error, "ignored a malformed toolbar cookie");
            Self::default()
        })
    }

    fn parse(value: &str) -> Result<Self> {
        let mut overrides = Self::default();
        let mut flags = Vec::new();
        for item in value.split('&').filter(|item| !item.is_empty()) {
            let (key, value) = item.split_once('=').ok_or_else(|| malformed(item))?;
            match key {
                "as" => {
                    let (area, principal) = value.split_once(':').ok_or_else(|| malformed(item))?;
                    overrides.acting_as = Some(ActingAs {
                        area: token(area)?,
                        principal: principal.parse().map_err(|_| malformed(item))?,
                    });
                }
                "flag" => {
                    let (name, answer) = value.split_once(':').ok_or_else(|| malformed(item))?;
                    flags.push((token(name)?, token(answer)?));
                }
                "clock" => {
                    overrides.clock_seconds = Some(value.parse().map_err(|_| malformed(item))?);
                }
                _ => return Err(malformed(item)),
            }
        }
        overrides.flags = flags.into();
        Ok(overrides)
    }

    pub(crate) fn cookie(&self, cookie_name: &str) -> Result<HeaderValue> {
        let mut items = Vec::new();
        if let Some(acting) = &self.acting_as {
            items.push(format!("as={}:{}", acting.area, acting.principal));
        }
        for (name, answer) in self.flags.iter() {
            items.push(format!("flag={name}:{answer}"));
        }
        if let Some(seconds) = self.clock_seconds {
            items.push(format!("clock={seconds}"));
        }
        let value = match items.join("&") {
            items if items.is_empty() => "; Max-Age=0".to_owned(),
            items => format!("{items}{SEAL}{}", keys()?.sign(SIGNED, items.as_bytes())),
        };
        HeaderValue::try_from(format!("{cookie_name}={value}; {ATTRIBUTES}"))
            .map_err(|_| Error::internal_message("a toolbar cookie is not a header value"))
    }

    pub(crate) fn claims(&self, now: Timestamp) -> Option<SessionClaims> {
        self.acting_as.as_ref().map(|acting| {
            SessionClaims::new(Uuid::from_u128(0), &acting.area, acting.principal, now)
        })
    }
}

/// It names a principal to act as, so only this process's own toolbar may have written it.
const SIGNED: Signed = Signed::PrincipalToken;

/// The process's own: a restart forgets every browser's overrides.
fn keys() -> Result<&'static SigningKeys> {
    static KEYS: OnceLock<SigningKeys> = OnceLock::new();
    if let Some(keys) = KEYS.get() {
        return Ok(keys);
    }
    let generated = SigningKeys::generate()?;
    Ok(KEYS.get_or_init(|| generated))
}

fn unsealed(value: &str) -> Option<&str> {
    let (items, seal) = value.rsplit_once(SEAL)?;
    keys()
        .ok()?
        .verifies(SIGNED, items.as_bytes(), seal)
        .then_some(items)
}

/// Kept to what a cookie value carries unquoted.
pub(crate) fn token(text: &str) -> Result<String> {
    let allowed =
        |character: char| character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-');
    if text.is_empty() || !text.chars().all(allowed) {
        return Err(Error::BadRequest(format!(
            "`{text}` is not letters, digits, `_`, `.` and `-`"
        )));
    }
    Ok(text.to_owned())
}

fn malformed(item: &str) -> Error {
    Error::BadRequest(format!("the toolbar cookie's `{item}` is malformed"))
}

#[cfg(test)]
mod tests {
    use http::header::COOKIE as COOKIE_HEADER;

    use super::*;

    const COOKIE: &str = "__Host-shop-toolbar";

    #[test]
    fn overrides_come_back_from_the_cookie_they_set() {
        let overrides = Overrides {
            acting_as: Some(ActingAs {
                area: "members".to_owned(),
                principal: Uuid::from_u128(7),
            }),
            flags: Arc::new([("shop.beta".to_owned(), "off".to_owned())]),
            clock_seconds: Some(-3600),
        };
        let set = overrides.cookie(COOKIE).unwrap();
        let pair = set.to_str().unwrap().split(';').next().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(COOKIE_HEADER, HeaderValue::from_str(pair).unwrap());

        assert_eq!(Overrides::of(&headers, COOKIE), overrides);
    }

    #[test]
    fn a_malformed_or_unsealed_cookie_overrides_nothing() {
        let sealed = |value: &str| {
            let seal = keys().unwrap().sign(SIGNED, value.as_bytes());
            format!("{value}{SEAL}{seal}")
        };
        let malformed = ["as=members", "flag=a b:on", "clock=soon", "other=1"].map(sealed);
        let unsealed = ["clock=1".to_owned(), "clock=1&seal=AAAA".to_owned()];
        for value in malformed.iter().chain(&unsealed) {
            let mut headers = HeaderMap::new();
            let pair = format!("{COOKIE}={value}");
            headers.insert(COOKIE_HEADER, HeaderValue::from_str(&pair).unwrap());

            assert_eq!(
                Overrides::of(&headers, COOKIE),
                Overrides::default(),
                "{value}"
            );
        }
    }

    #[test]
    fn no_override_left_removes_the_cookie() {
        let set = Overrides::default().cookie(COOKIE).unwrap();

        assert!(set.to_str().unwrap().contains("Max-Age=0"));
    }
}
