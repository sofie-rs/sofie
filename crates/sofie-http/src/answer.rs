use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use http::header::{
    ALLOW, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, LOCATION, REFERRER_POLICY,
    RETRY_AFTER, STRICT_TRANSPORT_SECURITY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use http::{HeaderValue, StatusCode};
use http_body_util::Full;
use sofie_core::encoding::percent;
use sofie_core::{Error, FieldError, Result};
use sofie_db::Reply;

use crate::caller::Caller;
use crate::htmx;
#[cfg(feature = "engine")]
use crate::language::Language;

pub(crate) type Response = http::Response<Full<Bytes>>;

pub(crate) const HTML: &str = "text/html; charset=utf-8";
pub(crate) const JSON: &str = "application/json";
pub(crate) const TEXT: &str = "text/plain; charset=utf-8";
pub(crate) const PERMISSIONS_POLICY_HEADER: &str = "permissions-policy";

const CONFLICT: &str = "This record changed since you loaded it; reload and try again.";
const UNAVAILABLE: &str = "The service is busy; try again shortly.";
/// The value HSTS preload lists ask for.
const TRANSPORT_SECURITY: &str = "max-age=63072000; includeSubDomains";
/// For every answer without a policy of its own (JSON, errors, health): it loads nothing
/// and may not be framed, should a browser ever render it.
const BASELINE_CONTENT_SECURITY: &str = "default-src 'none'; frame-ancestors 'none'";
/// Payment is left alone for a provider's embedded form.
const PERMISSIONS_POLICY: &str = "accelerometer=(), browsing-topics=(), camera=(), \
    display-capture=(), geolocation=(), gyroscope=(), magnetometer=(), microphone=(), \
    midi=(), usb=()";

#[derive(Clone, Copy)]
pub(crate) enum Redirect {
    /// Post-redirect-get, so reloading the landing page does not post again.
    SeeOther,
    Navigate,
    Permanent,
}

/// `content_type` is `None` only for an empty body.
pub(crate) fn response(
    status: StatusCode,
    content_type: Option<&'static str>,
    body: impl Into<Bytes>,
) -> Response {
    let mut response = Response::new(Full::new(body.into()));
    *response.status_mut() = status;
    if let Some(content_type) = content_type {
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    }
    response
}

pub(crate) fn redirect(kind: Redirect, location: &str) -> Result<Response> {
    let location = HeaderValue::from_str(location)
        .map_err(|error| Error::internal("building the redirect", error))?;
    let (status, header) = match kind {
        Redirect::SeeOther => (StatusCode::SEE_OTHER, LOCATION),
        Redirect::Navigate => (StatusCode::NO_CONTENT, htmx::REDIRECT),
        Redirect::Permanent => (StatusCode::MOVED_PERMANENTLY, LOCATION),
    };
    let mut response = response(status, None, Bytes::new());
    response.headers_mut().insert(header, location);
    Ok(response)
}

pub(crate) enum Refusal {
    Message(StatusCode, String),
    Fields(Vec<FieldError>),
}

impl Refusal {
    pub(crate) fn of(error: Error) -> Self {
        let message = |status, text: &str| Self::Message(status, text.to_owned());
        match error {
            Error::NotFound | Error::Moved { .. } => message(StatusCode::NOT_FOUND, "Not found."),
            Error::Conflict => message(StatusCode::CONFLICT, CONFLICT),
            Error::BadRequest(text) => Self::Message(StatusCode::BAD_REQUEST, text),
            Error::Invalid(fields) => Self::Fields(fields),
            Error::Rule(text) => Self::Message(StatusCode::UNPROCESSABLE_ENTITY, text),
            Error::TooManyRequests(wait) => Self::Message(
                StatusCode::TOO_MANY_REQUESTS,
                format!(
                    "Too many requests; try again in {} seconds.",
                    whole_seconds(wait)
                ),
            ),
            Error::Unavailable(reason) => {
                tracing::warn!(error.kind = "unavailable", error = %reason, "the request was turned away");
                message(StatusCode::SERVICE_UNAVAILABLE, UNAVAILABLE)
            }
            internal @ Error::Internal { .. } => {
                tracing::error!(error.kind = "internal", error = %internal, "the request failed");
                message(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong.")
            }
        }
    }

    pub(crate) fn status(&self) -> StatusCode {
        match self {
            Self::Message(status, _) => *status,
            Self::Fields(_) => StatusCode::UNPROCESSABLE_ENTITY,
        }
    }

    #[cfg(feature = "engine")]
    pub(crate) fn field_errors(&self, language: Language<'_>) -> Vec<(&str, String)> {
        match self {
            Self::Message(..) => Vec::new(),
            Self::Fields(fields) => fields
                .iter()
                .map(|field| (field.field.as_str(), language.field_error(field)))
                .collect(),
        }
    }

    fn answer(self, caller: Caller<'_>) -> Response {
        match caller {
            #[cfg(feature = "engine")]
            Caller::Htmx { trigger, .. } => self.notice(trigger, None, Language::plain()),
            Caller::Page | Caller::FormPost => self.page(),
            _ => self.json(),
        }
    }

    /// Plain and self-contained, so the baseline policy that loads nothing still shows it.
    fn page(&self) -> Response {
        let title = self
            .status()
            .canonical_reason()
            .unwrap_or("Refused")
            .to_owned();
        let mut body = String::with_capacity(256);
        body.push_str("<!doctype html><html><head><meta charset=\"utf-8\"><title>");
        push_html_escaped(&mut body, &title);
        body.push_str("</title></head><body><main><h1>");
        push_html_escaped(&mut body, &title);
        body.push_str("</h1>");
        match self {
            Self::Message(_, text) => {
                body.push_str("<p>");
                push_html_escaped(&mut body, text);
                body.push_str("</p>");
            }
            Self::Fields(fields) => {
                body.push_str("<ul>");
                for field in fields {
                    body.push_str("<li>");
                    push_html_escaped(&mut body, &field.field);
                    body.push_str(": ");
                    push_html_escaped(&mut body, &field.message);
                    body.push_str("</li>");
                }
                body.push_str("</ul>");
            }
        }
        body.push_str("</main></body></html>");
        response(self.status(), Some(HTML), body)
    }

    /// Written straight into the body: refusals are what floods and scanners are answered with.
    fn json(&self) -> Response {
        let mut body = Vec::with_capacity(64);
        match self {
            Self::Message(_, text) => {
                body.extend_from_slice(b"{\"error\":");
                push_json_string(&mut body, text);
            }
            Self::Fields(fields) => {
                body.extend_from_slice(b"{\"errors\":[");
                for (index, field) in fields.iter().enumerate() {
                    if index > 0 {
                        body.push(b',');
                    }
                    push_json_field_error(&mut body, field);
                }
                body.push(b']');
            }
        }
        body.push(b'}');
        response(self.status(), Some(JSON), body)
    }
}

fn push_html_escaped(body: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => body.push_str("&amp;"),
            '<' => body.push_str("&lt;"),
            '>' => body.push_str("&gt;"),
            '"' => body.push_str("&quot;"),
            '\'' => body.push_str("&#39;"),
            other => body.push(other),
        }
    }
}

/// `key` and `arguments` travel so a calling app can translate the refusal in its own words.
fn push_json_field_error(body: &mut Vec<u8>, field: &FieldError) {
    body.extend_from_slice(b"{\"field\":");
    push_json_string(body, &field.field);
    body.extend_from_slice(b",\"key\":");
    push_json_string(body, &field.key);
    body.extend_from_slice(b",\"arguments\":{");
    for (index, (name, value)) in field.arguments.iter().enumerate() {
        if index > 0 {
            body.push(b',');
        }
        push_json_string(body, name);
        body.push(b':');
        push_json_string(body, value);
    }
    body.extend_from_slice(b"},\"message\":");
    push_json_string(body, &field.message);
    body.push(b'}');
}

fn push_json_string(body: &mut Vec<u8>, text: &str) {
    serde_json::to_writer(body, text).expect("a string always serializes into a Vec");
}

pub(crate) fn outcome(result: Result<Reply>, caller: Caller<'_>) -> Response {
    match result {
        Ok(Reply::Empty) => response(StatusCode::NO_CONTENT, None, Bytes::new()),
        Ok(reply) => or_failure(json_body(&reply), caller),
        Err(error) => failure(error, caller),
    }
}

/// A document route's loader answers its text; anything else is the route's defect.
pub(crate) fn document(
    result: Result<Reply>,
    media: crate::routes::Media,
    caller: Caller<'_>,
) -> Response {
    #[cfg(feature = "csv")]
    if media == crate::routes::Media::Csv {
        let csv = result.and_then(|reply| match reply.shown() {
            Some(shown) => crate::csv::render(shown),
            None => crate::csv::render(sofie_core::Show::show(&reply.to_json()?)),
        });
        let csv = csv.map(|csv| response(StatusCode::OK, Some(media.content_type()), csv));
        return or_failure(csv, caller);
    }
    let text = result
        .and_then(Reply::into_json)
        .and_then(|value| match value {
            serde_json::Value::String(text) => Ok(text),
            other => Err(Error::internal_message(format!(
                "a document route's loader answered {}, not text",
                json_kind(&other)
            ))),
        });
    let text = text.map(|text| response(StatusCode::OK, Some(media.content_type()), text));
    or_failure(text, caller)
}

fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "nothing",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "text",
        serde_json::Value::Array(_) => "a list",
        serde_json::Value::Object(_) => "an object",
    }
}

/// Answers are mostly alike in size, so starting at the last length rarely regrows.
static JSON_CAPACITY: AtomicUsize = AtomicUsize::new(0);
/// So one large answer does not make every answer after it reserve as much.
const JSON_CAPACITY_LIMIT: usize = 64 * 1024;

fn json_starting_capacity() -> usize {
    JSON_CAPACITY
        .load(Ordering::Relaxed)
        .min(JSON_CAPACITY_LIMIT)
}

fn json_body(reply: &Reply) -> Result<Response> {
    let mut body = Vec::with_capacity(json_starting_capacity());
    reply.write_json(&mut body)?;
    JSON_CAPACITY.store(body.len(), Ordering::Relaxed);
    Ok(response(StatusCode::OK, Some(JSON), body))
}

#[cfg(all(feature = "testing", debug_assertions))]
const TEST_ERROR: &str = "x-sofie-test-error";

pub(crate) fn failure(error: Error, caller: Caller<'_>) -> Response {
    let retry_after = match &error {
        Error::TooManyRequests(wait) => Some(whole_seconds(*wait)),
        Error::Unavailable(_) => Some(1),
        _ => None,
    };
    #[cfg(all(feature = "testing", debug_assertions))]
    let internal = matches!(error, Error::Internal { .. }).then(|| error.to_string());
    let mut response = Refusal::of(error).answer(caller);
    #[cfg(all(feature = "testing", debug_assertions))]
    if let Some(internal) = internal {
        let shown: String = internal
            .chars()
            .take(2000)
            .map(|character| {
                if character == ' ' || character.is_ascii_graphic() {
                    character
                } else {
                    '?'
                }
            })
            .collect();
        if let Ok(value) = HeaderValue::from_str(&shown) {
            response.headers_mut().insert(TEST_ERROR, value);
        }
    }
    if let Some(seconds) = retry_after {
        response
            .headers_mut()
            .insert(RETRY_AFTER, HeaderValue::from(seconds));
    }
    response
}

/// Weak comparison (RFC 9110, 13.1.2): compressed and identity bodies share a tag.
#[cfg(any(feature = "etag", feature = "storage"))]
pub(crate) fn is_held(request: &http::HeaderMap, tag: &str) -> bool {
    fn opaque(tag: &str) -> &str {
        tag.trim().trim_start_matches("W/")
    }
    request
        .get_all(http::header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|held| held.trim() == "*" || opaque(held) == opaque(tag))
}

pub(crate) fn whole_seconds(wait: Duration) -> u64 {
    wait.as_secs() + u64::from(wait.subsec_nanos() > 0)
}

/// To the request's own path with the last segment that named `from` naming `to`, its query
/// kept; a request whose path never named `from`, or that a browser would read as another
/// host's (`/\evil.test/old`), is answered as not found.
pub(crate) fn moved(uri: &http::Uri, from: &str, to: &str, caller: Caller<'_>) -> Response {
    let mut segments: Vec<&str> = uri.path().split('/').collect();
    let named = segments.iter().rposition(|segment| {
        percent::decode(segment.as_bytes()).is_some_and(|decoded| decoded == from.as_bytes())
    });
    let Some(named) = named else {
        return failure(Error::NotFound, caller);
    };
    let encoded = percent::encode(to, percent::is_unreserved);
    segments[named] = &encoded;
    let mut location = segments.join("/");
    if !crate::guard::is_local_path(&location) {
        return failure(Error::NotFound, caller);
    }
    if let Some(query) = uri.query() {
        location.push('?');
        location.push_str(query);
    }
    or_failure(redirect(Redirect::Permanent, &location), caller)
}

pub(crate) fn or_failure(result: Result<Response>, caller: Caller<'_>) -> Response {
    result.unwrap_or_else(|error| failure(error, caller))
}

pub(crate) fn refusal(status: StatusCode, text: &str, caller: Caller<'_>) -> Response {
    Refusal::Message(status, text.to_owned()).answer(caller)
}

pub(crate) fn method_not_allowed(allowed: &[&str], caller: Caller<'_>) -> Response {
    let allow = match HeaderValue::from_str(&allowed.join(", ")) {
        Ok(allow) => allow,
        Err(error) => return failure(Error::internal("building the allow header", error), caller),
    };
    let mut response = refusal(
        StatusCode::METHOD_NOT_ALLOWED,
        "Method not allowed.",
        caller,
    );
    response.headers_mut().insert(ALLOW, allow);
    response
}

/// Browsers ignore HSTS over plain HTTP, so it is safe before TLS terminates.
pub(crate) fn secure(mut response: Response) -> Response {
    let is_not_modified = response.status() == StatusCode::NOT_MODIFIED;
    let headers = response.headers_mut();
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        STRICT_TRANSPORT_SECURITY,
        HeaderValue::from_static(TRANSPORT_SECURITY),
    );
    headers.insert(
        PERMISSIONS_POLICY_HEADER,
        HeaderValue::from_static(PERMISSIONS_POLICY),
    );
    headers
        .entry(CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    // A 304's headers replace the cached page's, whose nonce policy must survive.
    if !is_not_modified {
        headers
            .entry(CONTENT_SECURITY_POLICY)
            .or_insert(HeaderValue::from_static(BASELINE_CONTENT_SECURITY));
    }
    response
}

#[cfg(feature = "engine")]
impl Refusal {
    pub(crate) fn messages(&self, language: Language<'_>) -> Vec<String> {
        match self {
            Self::Message(_, text) => vec![text.clone()],
            Self::Fields(fields) => fields.iter().map(|field| told(field, language)).collect(),
        }
    }

    /// htmx swaps an error status too (see the engine's htmx config). A field error goes under
    /// its field among the form's `shown` fields, or else is told as text; `None` places all.
    pub(crate) fn notice(
        &self,
        form: Option<&str>,
        shown: Option<&[&str]>,
        language: Language<'_>,
    ) -> Response {
        let html = match (self, form) {
            (Self::Fields(fields), Some(form)) => fields_notice(fields, form, shown, language),
            _ => sofie_engine::notice(&self.messages(language)),
        };
        let mut response = response(self.status(), Some(HTML), html);
        let headers = response.headers_mut();
        headers.insert(
            htmx::RETARGET,
            HeaderValue::from_static(sofie_engine::NOTICE_TARGET),
        );
        headers.insert(htmx::RESWAP, HeaderValue::from_static("innerHTML"));
        response
    }
}

/// A JSON caller reads a refused value in its language too, as a page would show it.
#[cfg(feature = "engine")]
pub(crate) fn in_language(result: Result<Reply>, language: Language<'_>) -> Result<Reply> {
    result.map_err(|error| match error {
        Error::Invalid(fields) => Error::Invalid(
            fields
                .into_iter()
                .map(|field| FieldError {
                    message: language.field_error(&field),
                    ..field
                })
                .collect(),
        ),
        other => other,
    })
}

#[cfg(feature = "engine")]
fn told(field: &FieldError, language: Language<'_>) -> String {
    format!(
        "{}: {}",
        language.label(&field.field),
        language.field_error(field)
    )
}

#[cfg(feature = "engine")]
fn fields_notice(
    fields: &[FieldError],
    form: &str,
    shown: Option<&[&str]>,
    language: Language<'_>,
) -> String {
    let (placed, unplaced): (Vec<&FieldError>, Vec<&FieldError>) = fields
        .iter()
        .partition(|field| shown.is_none_or(|shown| shown.contains(&field.field.as_str())));
    let mut html = String::new();
    if !placed.is_empty() {
        html.push_str(&language.fields_notice(placed.len()));
    }
    let told: Vec<String> = unplaced.iter().map(|field| told(field, language)).collect();
    html.push_str(&sofie_engine::notice(&told));
    let errors: Vec<(&str, String)> = placed
        .iter()
        .map(|field| (field.field.as_str(), language.field_error(field)))
        .collect();
    html.push_str(&sofie_engine::field_errors(form, &errors));
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_large_json_answer_does_not_size_the_ones_after_it() {
        let large = serde_json::Value::String("a".repeat(4 * JSON_CAPACITY_LIMIT));

        json_body(&Reply::Json(large)).unwrap();

        assert!(json_starting_capacity() <= JSON_CAPACITY_LIMIT);
    }

    #[test]
    fn a_moved_record_redirects_within_the_site_only() {
        let moved_to = |path: &str| {
            let uri: http::Uri = path.parse().unwrap();
            moved(&uri, "old", "new", Caller::Browser)
        };

        let local = moved_to("/orders/old?tab=lines");
        assert_eq!(local.status(), StatusCode::MOVED_PERMANENTLY);
        assert_eq!(
            local.headers()[http::header::LOCATION],
            "/orders/new?tab=lines"
        );
        for off_site in ["/\\evil.test/old", "//evil.test/old"] {
            assert_eq!(
                moved_to(off_site).status(),
                StatusCode::NOT_FOUND,
                "{off_site}"
            );
        }
    }

    #[test]
    fn a_browser_navigating_is_refused_with_a_page_and_anyone_else_with_json() {
        let page = failure(Error::NotFound, Caller::Page);
        let rule = failure(Error::Rule("<b>A & B</b>".to_owned()), Caller::Page);
        let other = failure(Error::NotFound, Caller::Browser);

        assert_eq!(page.status(), StatusCode::NOT_FOUND);
        assert_eq!(page.headers()[CONTENT_TYPE], HTML);
        let body = |response: Response| {
            String::from_utf8(response.into_body().into_inner().unwrap().to_vec()).unwrap()
        };
        let page = body(page);
        assert!(
            page.contains("<h1>Not Found</h1><p>Not found.</p>"),
            "{page}"
        );
        let rule = body(rule);
        assert!(
            rule.contains("<p>&lt;b&gt;A &amp; B&lt;/b&gt;</p>"),
            "{rule}"
        );
        assert_eq!(other.headers()[CONTENT_TYPE], JSON);
    }

    #[test]
    fn a_field_error_is_answered_in_json_with_its_key_and_arguments() {
        let refused =
            FieldError::new("code", "validation.max_length", "must be at most 4").with("max", "4");

        let response = failure(Error::Invalid(vec![refused]), Caller::Json);

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body: serde_json::Value =
            serde_json::from_slice(&response.into_body().into_inner().unwrap()).unwrap();
        assert_eq!(
            body,
            serde_json::json!({ "errors": [{
                "field": "code",
                "key": "validation.max_length",
                "arguments": { "max": "4" },
                "message": "must be at most 4",
            }] })
        );
    }

    #[test]
    fn a_form_posted_without_scripts_is_refused_with_a_page() {
        let response = failure(
            Error::BadRequest("The form could not be read.".to_owned()),
            Caller::FormPost,
        );

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()[CONTENT_TYPE], HTML);
    }

    #[test]
    fn a_busy_resource_is_answered_503_with_when_to_try_again() {
        let response = failure(
            Error::Unavailable("no database connection came free".to_owned()),
            Caller::Json,
        );

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[RETRY_AFTER], "1");
    }

    #[test]
    fn too_many_requests_is_answered_429_with_the_wait_rounded_up() {
        let response = failure(
            Error::TooManyRequests(Duration::from_millis(2_500)),
            Caller::Json,
        );

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[RETRY_AFTER], "3");
    }
}
