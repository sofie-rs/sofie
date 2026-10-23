use http::request::Parts;
use http::{Method, StatusCode};
use sofie_core::Result;

use crate::answer::{self, Redirect, Response};
use crate::caller::Caller;
use crate::router::{Lookup, Router};
use crate::routes::Audience;
use crate::session::RequestSession;
use crate::urlencoded::{self, percent_encode};
use crate::{Access, Area, RequestContext};
use sofie_db::Resolve;

const SIGN_IN_FIRST: &str = "Sign in to continue.";

pub(crate) fn unauthenticated(audience: Audience, request: &Parts, caller: Caller<'_>) -> Response {
    let location = sign_in_location(audience.sign_in, request);
    let redirect = match caller {
        Caller::Json => return answer::refusal(StatusCode::UNAUTHORIZED, SIGN_IN_FIRST, caller),
        #[cfg(feature = "clients")]
        Caller::Screen => return answer::refusal(StatusCode::UNAUTHORIZED, SIGN_IN_FIRST, caller),
        Caller::Htmx { .. } => {
            answer::redirect(Redirect::Navigate, &location).map(|mut response| {
                *response.status_mut() = StatusCode::UNAUTHORIZED;
                response
            })
        }
        Caller::FormPost | Caller::Page | Caller::Browser => {
            answer::redirect(Redirect::SeeOther, &location)
        }
    };
    answer::or_failure(redirect, caller)
}

/// Only a GET to a path of this site, so the sign-in page cannot redirect anyone elsewhere.
fn sign_in_location(sign_in: &str, request: &Parts) -> String {
    let wanted = request
        .uri
        .path_and_query()
        .map(http::uri::PathAndQuery::as_str)
        .filter(|path| request.method == Method::GET && is_local_path(path));
    with_next(sign_in, wanted)
}

fn with_next(action: &str, next: Option<&str>) -> String {
    match next {
        Some(path) => format!("{action}?next={}", percent_encode(path)),
        None => action.to_owned(),
    }
}

/// `//host` and `/\host` leave the site, also behind a tab or newline, which browsers drop.
pub(crate) fn is_local_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && !path.contains('\\')
        && !path.contains(char::is_control)
}

pub struct Next {
    path: Option<String>,
}

impl Next {
    #[must_use]
    pub fn carried_by(&self, action: &str) -> String {
        with_next(action, self.path.as_deref())
    }

    /// A local path, checked as when the browser is sent there after signing in.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }
}

impl<A: Area, M> Resolve<RequestContext<A, M>> for Next {
    fn resolve(context: &mut RequestContext<A, M>) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Ok(Self {
            path: next_path(context.query()),
        }))
    }
}

/// Decoding once more must not leave the site either, so an escaped `//`, `\` or scheme is refused.
fn next_path(query: Option<&str>) -> Option<String> {
    let raw = query?
        .split('&')
        .find_map(|pair| pair.strip_prefix("next="))?;
    let path = urlencoded::decode(raw.as_bytes()).ok()?;
    let again = urlencoded::decode(path.as_bytes()).ok()?;
    (path.is_ascii() && is_local_path(&path) && is_local_path(&again)).then_some(path)
}

/// A page of another area than the one just signed in to would only send it back to sign in.
pub(crate) fn signed_in_destination(
    request: &Parts,
    session: &RequestSession,
    router: &Router,
) -> Option<String> {
    let area = session.signed_in_area()?;
    let path = next_path(request.uri.query())?;
    let page = path.split('?').next().unwrap_or_default();
    let Lookup::Found(target, _) = router.find(&Method::GET, page) else {
        return None;
    };
    let audience = target.audience;
    let opens = audience.access == Access::Public || audience.name == area;
    opens.then_some(path)
}

#[cfg(test)]
mod tests {
    use http::Request;

    use super::*;

    fn location(method: Method, uri: &str) -> String {
        let (parts, ()) = Request::builder()
            .method(method)
            .uri(uri)
            .body(())
            .unwrap()
            .into_parts();
        sign_in_location("/sign-in", &parts)
    }

    #[test]
    fn a_page_is_named_as_the_next_path() {
        assert_eq!(
            location(Method::GET, "/account/orders?page=2"),
            "/sign-in?next=/account/orders%3Fpage%3D2"
        );
    }

    #[test]
    fn a_next_path_is_decoded_and_kept_only_on_this_site() {
        assert_eq!(
            next_path(Some("a=1&next=/account/orders%3Fpage%3D2")),
            Some("/account/orders?page=2".to_owned())
        );
        for refused in [
            "next=//evil.test",
            "next=/\\evil.test",
            "next=https://evil.test",
            "next=javascript:alert(1)",
            "next=%2F%2Fevil.test",
            "next=%2f%5Cevil.test",
            "next=/%2F/evil.test",
            "next=/%5Cevil.test",
            "next=%252F%252Fevil.test",
            "next=/%2F%252Fevil.test",
            "next=%09//evil.test",
            "next=/%09/evil.test",
            "next=/%E2%80%8B",
            "next=%",
        ] {
            assert_eq!(next_path(Some(refused)), None, "{refused}");
        }
        assert_eq!(next_path(None), None);
    }

    #[test]
    fn no_next_path_leaves_the_site_or_repeats_a_write() {
        assert_eq!(location(Method::GET, "//evil.test/x"), "/sign-in");
        assert_eq!(
            location(Method::GET, "https://evil.test/x"),
            "/sign-in?next=/x"
        );
        assert_eq!(location(Method::POST, "/account/orders"), "/sign-in");
        assert!(!is_local_path("/\\evil.test"));
        assert!(!is_local_path("evil.test"));
    }
}
