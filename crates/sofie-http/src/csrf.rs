use http::HeaderValue;
use http::header::{HOST, ORIGIN};
use http::request::Parts;

use crate::admission::AreaPolicy;
use crate::cookie::CookieNames;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Check {
    SecFetchSite,
    Origin,
    SessionWithoutOrigin,
}

impl Check {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::SecFetchSite => "sec-fetch-site",
            Self::Origin => "origin",
            Self::SessionWithoutOrigin => "session-without-origin",
        }
    }
}

/// Besides the app's own pages, from an origin the area's CORS policy admits writes from.
pub(crate) fn refused_in(
    #[cfg_attr(not(feature = "cors"), expect(unused_variables))] area: &AreaPolicy,
    request: &Parts,
    cookies: &CookieNames,
) -> Option<Check> {
    let check = refused_by(request, cookies)?;
    #[cfg(feature = "cors")]
    if area
        .cors
        .as_ref()
        .is_some_and(|cors| cors.admits_write(&request.headers, cookies))
    {
        return None;
    }
    Some(check)
}

/// Unsafe methods run only from the app's own pages (`Sec-Fetch-Site`, else `Origin`).
pub(crate) fn refused_by(request: &Parts, cookies: &CookieNames) -> Option<Check> {
    if request.method.is_safe() {
        return None;
    }
    let headers = &request.headers;
    if let Some(site) = headers.get("sec-fetch-site") {
        return (site != "same-origin").then_some(Check::SecFetchSite);
    }
    if let Some(origin) = headers.get(ORIGIN) {
        let same = host_of(request).is_some_and(|host| is_origin_of(origin, host));
        return (!same).then_some(Check::Origin);
    }
    // A non-browser request carrying the session cookie is refused, valid session or not.
    cookies
        .session_value(headers)
        .map(|_| Check::SessionWithoutOrigin)
}

/// HTTP/2 carries the host in the URI's authority, HTTP/1.1 in `Host`.
pub(crate) fn host_of(request: &Parts) -> Option<&str> {
    request
        .uri
        .authority()
        .map(http::uri::Authority::as_str)
        .or_else(|| request.headers.get(HOST)?.to_str().ok())
}

/// The scheme is not compared: behind a TLS-terminating proxy the server cannot know it.
pub(crate) fn is_origin_of(origin: &HeaderValue, host: &str) -> bool {
    origin
        .to_str()
        .ok()
        .and_then(|origin| {
            origin
                .strip_prefix("https://")
                .or_else(|| origin.strip_prefix("http://"))
        })
        .is_some_and(|authority| authority.eq_ignore_ascii_case(host))
}

#[cfg(test)]
mod tests {
    use http::{Method, Request};

    use super::{Check, CookieNames, refused_by};

    fn shop() -> CookieNames {
        CookieNames::of("shop").unwrap()
    }

    fn refusal(method: Method, headers: &[(&str, &str)]) -> Option<Check> {
        let mut request = Request::builder()
            .method(method)
            .uri("/orders")
            .header("host", "shop.test:8080");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let (parts, ()) = request.body(()).unwrap().into_parts();
        refused_by(&parts, &shop())
    }

    fn allowed(method: Method, headers: &[(&str, &str)]) -> bool {
        refusal(method, headers).is_none()
    }

    #[test]
    fn safe_methods_pass_from_anywhere() {
        assert!(allowed(Method::GET, &[("sec-fetch-site", "cross-site")]));
        assert!(allowed(Method::HEAD, &[("origin", "https://evil.test")]));
    }

    #[test]
    fn a_same_origin_fetch_passes_and_any_other_site_is_refused() {
        for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert!(allowed(
                method.clone(),
                &[("sec-fetch-site", "same-origin")]
            ));
            for site in ["cross-site", "same-site", "none"] {
                assert!(
                    !allowed(method.clone(), &[("sec-fetch-site", site)]),
                    "{site}"
                );
            }
        }
    }

    #[test]
    fn sec_fetch_site_decides_over_origin() {
        let cross = [
            ("sec-fetch-site", "cross-site"),
            ("origin", "http://shop.test:8080"),
        ];
        assert!(!allowed(Method::POST, &cross));
    }

    #[test]
    fn without_sec_fetch_site_the_origin_must_name_the_host() {
        assert!(allowed(
            Method::POST,
            &[("origin", "http://shop.test:8080")]
        ));
        assert!(allowed(
            Method::POST,
            &[("origin", "https://SHOP.test:8080")]
        ));
        for origin in [
            "https://evil.test",
            "http://shop.test",
            "http://shop.test:8080.evil.test",
            "null",
            "shop.test:8080",
        ] {
            assert!(!allowed(Method::POST, &[("origin", origin)]), "{origin}");
        }
    }

    #[test]
    fn a_client_sending_neither_header_without_a_session_passes() {
        assert!(allowed(Method::POST, &[]));
        assert!(allowed(Method::POST, &[("cookie", "theme=dark")]));
    }

    #[test]
    fn a_session_cookie_without_either_header_is_refused() {
        for cookie in ["__Host-shop-session=x", "theme=dark; __Host-shop-session=x"] {
            assert!(!allowed(Method::POST, &[("cookie", cookie)]), "{cookie}");
        }
        let same_origin = [
            ("cookie", "__Host-shop-session=x"),
            ("sec-fetch-site", "same-origin"),
        ];
        assert!(allowed(Method::POST, &same_origin));
        assert!(allowed(Method::GET, &[("cookie", "__Host-shop-session=x")]));
    }

    #[test]
    fn each_refusal_names_its_check() {
        let refused = |headers: &[(&str, &str)]| refusal(Method::POST, headers);

        assert_eq!(
            refused(&[("sec-fetch-site", "cross-site")]),
            Some(Check::SecFetchSite)
        );
        assert_eq!(
            refused(&[("origin", "https://evil.test")]),
            Some(Check::Origin)
        );
        assert_eq!(
            refused(&[("cookie", "__Host-shop-session=x")]),
            Some(Check::SessionWithoutOrigin)
        );
    }

    #[test]
    fn an_http2_request_matches_the_origin_against_its_authority() {
        let (parts, ()) = Request::post("https://shop.test/orders")
            .header("origin", "https://shop.test")
            .body(())
            .unwrap()
            .into_parts();
        assert!(refused_by(&parts, &shop()).is_none());
    }
}
