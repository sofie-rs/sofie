use http::HeaderMap;
use http::header::ACCEPT;

use crate::htmx;
use crate::urlencoded::Encoding;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Caller<'r> {
    /// Anyone naming JSON before HTML in `Accept`.
    Json,
    Htmx {
        target: Option<&'r str>,
        trigger: Option<&'r str>,
    },
    FormPost,
    /// A browser navigating: `Accept` names `text/html`, so a refusal is a page too.
    Page,
    Browser,
    /// A client area's runtime, asking for a screen's envelope.
    #[cfg(feature = "clients")]
    Screen,
}

#[cfg(feature = "clients")]
pub(crate) const SCREEN_MEDIA_TYPE: &str = "application/vnd.sofie.screen+json";

impl<'r> Caller<'r> {
    pub(crate) fn of(headers: &'r HeaderMap) -> Self {
        #[cfg(feature = "clients")]
        if headers
            .get_all(ACCEPT)
            .iter()
            .any(|value| value.as_bytes() == SCREEN_MEDIA_TYPE.as_bytes())
        {
            return Self::Screen;
        }
        let accepted = Accepted::of(headers);
        if accepted.prefers_json() {
            return Self::Json;
        }
        if headers
            .get(htmx::REQUEST)
            .is_some_and(|value| value == "true")
        {
            return Self::Htmx {
                target: non_empty(headers, &htmx::TARGET),
                trigger: non_empty(headers, &htmx::TRIGGER),
            };
        }
        if Encoding::of(headers) == Encoding::Form {
            return Self::FormPost;
        }
        #[cfg(feature = "uploads")]
        if crate::multipart::boundary(headers).is_some() {
            return Self::FormPost;
        }
        if accepted.html.is_some_and(|(quality, _)| quality > 0.0) {
            return Self::Page;
        }
        Self::Browser
    }
}

fn non_empty<'r>(headers: &'r HeaderMap, name: &http::HeaderName) -> Option<&'r str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
}

/// Each type's quality and position in `Accept`.
struct Accepted {
    json: Option<(f32, std::cmp::Reverse<usize>)>,
    html: Option<(f32, std::cmp::Reverse<usize>)>,
}

impl Accepted {
    fn of(headers: &HeaderMap) -> Self {
        let mut accepted = Self {
            json: None,
            html: None,
        };
        let ranges = headers
            .get_all(ACCEPT)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','));
        for (position, range) in ranges.enumerate() {
            let mut parts = range.split(';');
            let name = parts.next().unwrap_or_default().trim();
            let ranked = Some((quality(parts), std::cmp::Reverse(position)));
            if accepted.json.is_none() && name.eq_ignore_ascii_case("application/json") {
                accepted.json = ranked;
            } else if accepted.html.is_none() && name.eq_ignore_ascii_case("text/html") {
                accepted.html = ranked;
            }
        }
        accepted
    }

    /// A browser ranks `text/html` above JSON; a tie goes to the type named first.
    fn prefers_json(&self) -> bool {
        self.json.is_some_and(|(quality, _)| quality > 0.0) && self.json > self.html
    }
}

/// The scheme is case-insensitive (RFC 9110 section 11.1).
#[cfg(any(feature = "tokens", feature = "metrics", test))]
pub(crate) fn bearer_credentials(authorization: &http::HeaderValue) -> Option<&str> {
    let (scheme, token) = authorization.to_str().ok()?.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

pub(crate) fn quality<'p>(mut parameters: impl Iterator<Item = &'p str>) -> f32 {
    parameters
        .find_map(|parameter| {
            let (key, value) = parameter.split_once('=')?;
            key.trim()
                .eq_ignore_ascii_case("q")
                .then(|| value.trim().parse().ok())?
        })
        .unwrap_or(1.0)
}

#[cfg(test)]
mod tests {
    use http::{HeaderName, HeaderValue};

    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(name, value)| {
                (
                    HeaderName::from_static(name),
                    HeaderValue::from_static(value),
                )
            })
            .collect()
    }

    const FORM: (&str, &str) = ("content-type", "application/x-www-form-urlencoded");

    #[test]
    fn json_is_asked_for_only_when_named_before_html() {
        assert_eq!(
            Caller::of(&headers(&[("accept", "application/json")])),
            Caller::Json
        );
        assert_eq!(
            Caller::of(&headers(&[("accept", "application/json, text/html")])),
            Caller::Json
        );
        let browser = "text/html,application/xhtml+xml,application/json;q=0.9,*/*;q=0.8";
        assert_eq!(Caller::of(&headers(&[("accept", browser)])), Caller::Page);
        assert_eq!(Caller::of(&headers(&[])), Caller::Browser);
        assert_eq!(
            Caller::of(&headers(&[("accept", "*/*, text/html;q=0")])),
            Caller::Browser
        );
    }

    #[test]
    fn a_bearer_scheme_is_matched_in_any_case() {
        let credentials =
            |value| bearer_credentials(&HeaderValue::from_static(value)).map(str::to_owned);

        assert_eq!(credentials("Bearer abc").as_deref(), Some("abc"));
        assert_eq!(credentials("bearer abc").as_deref(), Some("abc"));
        assert_eq!(credentials("Basic abc"), None);
        assert_eq!(credentials("Bearerabc"), None);
    }

    #[test]
    fn json_is_ranked_by_quality_and_case_is_ignored() {
        let json =
            |accept: &'static str| Accepted::of(&headers(&[("accept", accept)])).prefers_json();

        assert!(json("text/html;q=0.5, application/json"));
        assert!(json("Application/JSON"));
        assert!(!json("application/json;q=0"));
        assert!(!json("application/json;q=0.4, text/html;q=0.8"));
        assert!(!json("application/jsonx"));
    }

    #[test]
    fn htmx_asking_for_json_is_a_json_caller() {
        let json = headers(&[("hx-request", "true"), ("accept", "application/json")]);
        assert_eq!(Caller::of(&json), Caller::Json);
        assert_eq!(
            Caller::of(&headers(&[("hx-request", "false")])),
            Caller::Browser
        );
    }

    #[test]
    fn htmx_names_its_target_and_trigger_when_not_empty() {
        let htmx = headers(&[
            ("hx-request", "true"),
            ("hx-target", "orders"),
            ("hx-trigger", ""),
        ]);
        assert_eq!(
            Caller::of(&htmx),
            Caller::Htmx {
                target: Some("orders"),
                trigger: None
            }
        );
    }

    #[test]
    fn a_form_post_is_a_form_body_from_neither_htmx_nor_a_json_client() {
        assert_eq!(Caller::of(&headers(&[FORM])), Caller::FormPost);
        assert!(matches!(
            Caller::of(&headers(&[FORM, ("hx-request", "true")])),
            Caller::Htmx { .. }
        ));
        assert_eq!(
            Caller::of(&headers(&[FORM, ("accept", "application/json")])),
            Caller::Json
        );
        assert_eq!(
            Caller::of(&headers(&[("content-type", "application/json")])),
            Caller::Browser
        );
    }
}
