use std::time::Duration;

use bytes::Bytes;
use http::header::{
    ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
    ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_MAX_AGE,
    ACCESS_CONTROL_REQUEST_HEADERS, ACCESS_CONTROL_REQUEST_METHOD, ORIGIN, VARY,
};
use http::request::Parts;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use sofie_core::{Error, Result};

use crate::App;
use crate::answer::{self, Response};
use crate::cookie::CookieNames;
use crate::pipeline::Asked;
use crate::router::{Lookup, Router};
use crate::target::Target;

const ANY_ORIGIN: &str = "*";
const SETTING: &str = "SOFIE_CORS_ORIGINS";

/// Allows nothing until told; methods default to `GET`, `HEAD` and `POST`.
#[derive(Clone, Debug, Default)]
pub struct Cors {
    origins: Vec<String>,
    methods: Vec<Method>,
    headers: Vec<String>,
    exposed: Vec<String>,
    credentials: bool,
    max_age: Option<Duration>,
}

impl Cors {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Exactly `scheme://host[:port]`, or `*` for any origin without credentials.
    #[must_use]
    pub fn origin(mut self, origin: &str) -> Self {
        self.origins.push(origin.to_owned());
        self
    }

    #[must_use]
    pub fn methods(mut self, methods: &[Method]) -> Self {
        self.methods.extend_from_slice(methods);
        self
    }

    #[must_use]
    pub fn headers(mut self, names: &[&str]) -> Self {
        self.headers
            .extend(names.iter().map(|name| (*name).to_owned()));
        self
    }

    #[must_use]
    pub fn exposed_headers(mut self, names: &[&str]) -> Self {
        self.exposed
            .extend(names.iter().map(|name| (*name).to_owned()));
        self
    }

    /// Cookies ride along, so a write from an allowed origin passes the CSRF check with its session.
    #[must_use]
    pub fn credentials(mut self) -> Self {
        self.credentials = true;
        self
    }

    #[must_use]
    pub fn max_age(mut self, age: Duration) -> Self {
        self.max_age = Some(age);
        self
    }
}

/// `area=origin` pairs separated by commas, an area named once per origin.
#[derive(Clone, Debug, Default)]
pub struct CorsOrigins {
    origins: Vec<(String, HeaderValue)>,
}

impl CorsOrigins {
    pub fn parse(text: &str) -> Result<Self> {
        let origins = text
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| {
                let (area, origin) = entry.split_once('=').ok_or_else(|| {
                    Error::internal_message(format!("`{entry}` is not `area=origin`"))
                })?;
                Ok((area.trim().to_owned(), exact_origin(origin.trim())?))
            })
            .collect::<Result<_>>()?;
        Ok(Self { origins })
    }
}

impl crate::Serving {
    /// `sofie::run` reads them from `SOFIE_CORS_ORIGINS`; they add to each area's `App::cors` origins.
    #[must_use]
    pub fn cors_origins(mut self, origins: CorsOrigins) -> Self {
        self.cors_origins = origins;
        self
    }
}

pub(crate) struct AreaCors {
    origins: Origins,
    methods: Vec<Method>,
    allow_methods: HeaderValue,
    headers: Vec<HeaderName>,
    allow_headers: Option<HeaderValue>,
    exposed: Option<HeaderValue>,
    credentials: bool,
    max_age: Option<HeaderValue>,
}

enum Origins {
    Any,
    Listed(Vec<HeaderValue>),
}

pub(crate) fn prepare(app: &mut App) -> Result<()> {
    let mut configured = std::mem::take(&mut app.serving.cors_origins).origins;
    for (area, cors) in std::mem::take(&mut app.cors) {
        let from_settings = configured
            .extract_if(.., |(named, _)| named == area)
            .map(|(_, origin)| origin)
            .collect();
        let prepared = AreaCors::new(cors, from_settings)
            .map_err(|error| Error::internal(format!("the CORS policy of area `{area}`"), error))?;
        app.areas.entry(area).or_default().cors = Some(prepared);
    }
    match configured.first() {
        Some((area, _)) => Err(Error::internal_message(format!(
            "{SETTING} names area `{area}`, which has no `App::cors` policy"
        ))),
        None => Ok(()),
    }
}

impl AreaCors {
    fn new(cors: Cors, from_settings: Vec<HeaderValue>) -> Result<Self> {
        let origins = if cors.origins.iter().any(|origin| origin == ANY_ORIGIN) {
            if cors.credentials || cors.origins.len() > 1 || !from_settings.is_empty() {
                return Err(Error::internal_message(
                    "`*` must be the only origin, and allows no credentials",
                ));
            }
            Origins::Any
        } else {
            let mut listed: Vec<HeaderValue> = cors
                .origins
                .iter()
                .map(|origin| exact_origin(origin))
                .collect::<Result<_>>()?;
            listed.extend(from_settings);
            Origins::Listed(listed)
        };
        let methods = if cors.methods.is_empty() {
            vec![Method::GET, Method::HEAD, Method::POST]
        } else {
            cors.methods
        };
        let headers = header_names(&cors.headers)?;
        let exposed = header_names(&cors.exposed)?;
        Ok(Self {
            origins,
            allow_methods: joined(methods.iter().map(Method::as_str))?
                .ok_or_else(|| Error::internal_message("no methods"))?,
            methods,
            allow_headers: joined(headers.iter().map(HeaderName::as_str))?,
            headers,
            exposed: joined(exposed.iter().map(HeaderName::as_str))?,
            credentials: cors.credentials,
            max_age: cors.max_age.map(|age| HeaderValue::from(age.as_secs())),
        })
    }

    fn allowed_origin(&self, request: &HeaderMap) -> Option<HeaderValue> {
        let origin = request.get(ORIGIN)?;
        match &self.origins {
            Origins::Any => Some(HeaderValue::from_static(ANY_ORIGIN)),
            Origins::Listed(listed) => listed.contains(origin).then(|| origin.clone()),
        }
    }

    /// A write carrying the session cookie needs `credentials()`, the area's consent to it.
    pub(crate) fn admits_write(&self, request: &HeaderMap, cookies: &CookieNames) -> bool {
        self.allowed_origin(request).is_some()
            && (self.credentials || cookies.session_value(request).is_none())
    }

    fn allows_requested_headers(&self, request: &HeaderMap) -> bool {
        request
            .get_all(ACCESS_CONTROL_REQUEST_HEADERS)
            .iter()
            .all(|value| {
                value.to_str().is_ok_and(|names| {
                    names
                        .split(',')
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .all(|name| {
                            self.headers
                                .iter()
                                .any(|allowed| allowed.as_str().eq_ignore_ascii_case(name))
                        })
                })
            })
    }

    fn preflight(&self, method: &Method, asked: Asked<'_>) -> Response {
        let request = &asked.request.headers;
        let origin = self
            .allowed_origin(request)
            .filter(|_| self.methods.contains(method) && self.allows_requested_headers(request));
        let Some(origin) = origin else {
            tracing::warn!(
                http.request.method = method.as_str(),
                url.path = asked.request.uri.path(),
                "refused a cross-origin preflight"
            );
            let mut refused = answer::refusal(
                StatusCode::FORBIDDEN,
                "This origin may not call this route.",
                asked.caller,
            );
            self.vary(refused.headers_mut());
            return refused;
        };
        let mut response = answer::response(StatusCode::NO_CONTENT, None, Bytes::new());
        let headers = response.headers_mut();
        self.allow(origin, headers);
        headers.insert(ACCESS_CONTROL_ALLOW_METHODS, self.allow_methods.clone());
        if let Some(allowed) = &self.allow_headers {
            headers.insert(ACCESS_CONTROL_ALLOW_HEADERS, allowed.clone());
        }
        if let Some(age) = &self.max_age {
            headers.insert(ACCESS_CONTROL_MAX_AGE, age.clone());
        }
        response
    }

    fn allow(&self, origin: HeaderValue, headers: &mut HeaderMap) {
        self.vary(headers);
        headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        if self.credentials {
            headers.insert(
                ACCESS_CONTROL_ALLOW_CREDENTIALS,
                HeaderValue::from_static("true"),
            );
        }
    }

    /// An answer that differs by origin must not be served from a cache to another one.
    fn vary(&self, headers: &mut HeaderMap) {
        if matches!(self.origins, Origins::Listed(_)) {
            headers.append(VARY, HeaderValue::from_static("Origin"));
        }
    }
}

/// A route its flag hides is answered as a missing one, as `Allow` leaves it out.
pub(crate) fn preflight(
    router: &Router,
    asked: Asked<'_>,
    hidden: impl Fn(&Target) -> bool,
) -> Option<Response> {
    let request = asked.request;
    if request.method != Method::OPTIONS || !request.headers.contains_key(ORIGIN) {
        return None;
    }
    let requested = request.headers.get(ACCESS_CONTROL_REQUEST_METHOD)?;
    let method = Method::from_bytes(requested.as_bytes()).ok()?;
    let Lookup::Found(target, _) = router.find(&method, request.uri.path()) else {
        return None;
    };
    if hidden(target) {
        return None;
    }
    let cors = target.admission.area.cors.as_ref()?;
    Some(cors.preflight(&method, asked))
}

pub(crate) fn answered(
    target: Option<&Target>,
    request: &Parts,
    mut response: Response,
) -> Response {
    let Some(cors) = target.and_then(|target| target.admission.area.cors.as_ref()) else {
        return response;
    };
    let headers = response.headers_mut();
    let Some(origin) = cors.allowed_origin(&request.headers) else {
        cors.vary(headers);
        return response;
    };
    cors.allow(origin, headers);
    if let Some(exposed) = &cors.exposed {
        headers.insert(ACCESS_CONTROL_EXPOSE_HEADERS, exposed.clone());
    }
    response
}

/// Browsers send an origin lowercased and without a path, so it is compared byte for byte.
fn exact_origin(text: &str) -> Result<HeaderValue> {
    let invalid = || {
        Error::internal_message(format!(
            "`{text}` is not an origin such as https://app.example.com"
        ))
    };
    let authority = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))
        .ok_or_else(invalid)?;
    let valid = !authority.is_empty()
        && !authority.contains(['@', '/', '?', '#'])
        && authority.parse::<http::uri::Authority>().is_ok();
    if !valid {
        return Err(invalid());
    }
    HeaderValue::try_from(text.to_ascii_lowercase()).map_err(|_| invalid())
}

fn header_names(names: &[String]) -> Result<Vec<HeaderName>> {
    names
        .iter()
        .map(|name| {
            HeaderName::try_from(name.as_str())
                .map_err(|error| Error::internal(format!("header name `{name}`"), error))
        })
        .collect()
}

fn joined<'n>(names: impl Iterator<Item = &'n str>) -> Result<Option<HeaderValue>> {
    let joined = names.collect::<Vec<_>>().join(", ");
    if joined.is_empty() {
        return Ok(None);
    }
    HeaderValue::try_from(joined)
        .map(Some)
        .map_err(|error| Error::internal("joining a CORS header", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_origin_is_exactly_a_scheme_and_a_host() {
        assert_eq!(
            exact_origin("https://App.example:8443").unwrap(),
            "https://app.example:8443"
        );
        for invalid in [
            "app.example",
            "https://",
            "https://app.example/",
            "https://app.example/path",
            "https://user@app.example",
            "ftp://app.example",
            "null",
            "*",
        ] {
            assert!(exact_origin(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn any_origin_is_alone_and_without_credentials() {
        assert!(AreaCors::new(Cors::new().origin("*"), Vec::new()).is_ok());
        for cors in [
            Cors::new().origin("*").credentials(),
            Cors::new().origin("*").origin("https://app.example"),
        ] {
            assert!(AreaCors::new(cors, Vec::new()).is_err());
        }
        let from_settings = vec![exact_origin("https://app.example").unwrap()];
        assert!(AreaCors::new(Cors::new().origin("*"), from_settings).is_err());
    }

    #[test]
    fn settings_name_an_origin_per_area_and_refuse_a_malformed_one() {
        let origins = CorsOrigins::parse(
            "api=https://a.example, api=http://b.example:8080, web=https://c.example",
        )
        .unwrap();
        assert_eq!(origins.origins.len(), 3);
        assert_eq!(origins.origins[1].0, "api");

        assert_eq!(CorsOrigins::parse("").unwrap().origins.len(), 0);
        assert!(CorsOrigins::parse("https://a.example").is_err());
        assert!(CorsOrigins::parse("api=https://a.example/").is_err());
    }
}
