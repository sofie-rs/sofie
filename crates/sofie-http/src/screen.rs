use http::header::{CONTENT_SECURITY_POLICY, VARY};
use http::{HeaderValue, StatusCode};
use serde_json::Value;
use sofie_core::{Error, Result, Show, Uuid};
use sofie_db::Reply;
use sofie_engine::{Page, RenderError, Submitted, Template};

use crate::answer::{self, HTML, Refusal, Response};
use crate::caller::Caller;
use crate::content_security::AreaContentSecurity;
use crate::language::Language;
use crate::pipeline::Asked;

/// A negotiated language adds `Accept-Language`.
pub(crate) const VARIES_BY: &str = "Accept, HX-Request, HX-Target";

#[derive(Debug, PartialEq, Eq)]
enum Wanted<'r> {
    Json,
    Html {
        fragment: Option<&'r str>,
    },
    #[cfg(feature = "clients")]
    Envelope,
}

pub(crate) struct ScreenPage<'a> {
    pub(crate) template: &'a Template,
    pub(crate) title: &'a str,
    pub(crate) language: Language<'a>,
    pub(crate) content_security: &'a AreaContentSecurity,
    #[cfg(feature = "islands")]
    pub(crate) islands: Option<sofie_engine::islands::IslandLinks>,
    #[cfg(feature = "clients")]
    pub(crate) client: Option<&'a sofie_engine::ClientBundle>,
    #[cfg(feature = "gates")]
    pub(crate) answer: &'a crate::view::RouteAnswer<'a>,
}

#[cfg(not(feature = "gates"))]
impl ScreenPage<'_> {
    fn rendering(&self) -> &Template {
        self.template
    }
}

pub(crate) fn answer(
    result: Result<Reply>,
    caller: Caller<'_>,
    screen: &ScreenPage<'_>,
) -> Response {
    let mut response = match wanted(caller) {
        Wanted::Json => {
            #[cfg(feature = "gates")]
            let result = screen.withheld(result);
            answer::outcome(answer::in_language(result, screen.language), caller)
        }
        #[cfg(feature = "clients")]
        Wanted::Envelope => crate::client_page::enveloped(screen, result),
        #[cfg(feature = "clients")]
        Wanted::Html { fragment: None } if let Some(bundle) = screen.client_bundle() => {
            answer::or_failure(crate::client_page::shell(screen, bundle, result), caller)
        }
        Wanted::Html { fragment } => answer::or_failure(
            read(screen, result, |data| match fragment {
                Some(id) => requested_fragment(screen, data, id),
                None => page(screen, data, &[], None),
            }),
            caller,
        ),
    };
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static(screen.language.varies_by()));
    response
}

/// A fragment the route names but the screen lacks is the app's defect.
pub(crate) fn refreshed(
    result: Result<Reply>,
    caller: Caller<'_>,
    screen: &ScreenPage<'_>,
    fragment: &str,
) -> Response {
    let rendered = read(screen, result, |data| {
        let html = screen
            .rendering()
            .render_fragment(data, fragment)
            .map_err(|error| defect(screen.template, error))?;
        let mut html = sofie_engine::with_cleared_notice(html);
        if let Caller::Htmx {
            trigger: Some(trigger),
            ..
        } = caller
            && trigger != fragment
        {
            html.push_str(&reset(screen, data, trigger)?);
        }
        with_policy(html, screen, &new_nonce()?)
    });
    answer::or_failure(rendered, caller)
}

pub(crate) fn refused_page(
    result: Result<Reply>,
    refusal: &Refusal,
    asked: Asked<'_>,
    screen: &ScreenPage<'_>,
    input: &Value,
) -> Response {
    let errors = refusal.field_errors(screen.language);
    let submitted = Submitted {
        action: asked
            .request
            .uri
            .path_and_query()
            .map_or("", http::uri::PathAndQuery::as_str),
        input,
        errors: &errors,
    };
    let notice = refusal.messages(screen.language);
    let rendered = read(screen, result, |data| {
        page(screen, data, &notice, Some(&submitted))
    })
    .map(|mut response| {
        *response.status_mut() = refusal.status();
        response
    });
    answer::or_failure(rendered, asked.caller)
}

/// Out of band, so a form that posted comes back empty with its error slots cleared.
fn reset(screen: &ScreenPage<'_>, data: &dyn Show, trigger: &str) -> Result<String> {
    let rendered = screen.rendering().render_out_of_band(data, trigger);
    match rendered {
        Ok(html) => Ok(html),
        Err(RenderError::FragmentNotFound { .. }) => Ok(sofie_engine::field_errors(trigger, &[])),
        Err(error) => Err(defect(screen.template, error)),
    }
}

fn wanted(caller: Caller<'_>) -> Wanted<'_> {
    match caller {
        Caller::Json => Wanted::Json,
        Caller::Htmx { target, .. } => Wanted::Html { fragment: target },
        Caller::FormPost | Caller::Page | Caller::Browser => Wanted::Html { fragment: None },
        #[cfg(feature = "clients")]
        Caller::Screen => Wanted::Envelope,
    }
}

#[cfg(feature = "clients")]
impl ScreenPage<'_> {
    /// A screen its area's bundle does not hold stays an HTML page.
    fn client_bundle(&self) -> Option<&sofie_engine::ClientBundle> {
        self.client
            .filter(|bundle| bundle.has_screen(self.template.name()))
    }
}

/// The request's mistake: 404, not a logged failure.
fn requested_fragment(screen: &ScreenPage<'_>, data: &dyn Show, id: &str) -> Result<Response> {
    let html = screen
        .rendering()
        .render_fragment(data, id)
        .map_err(|error| match error {
            RenderError::FragmentNotFound { .. } => Error::NotFound,
            RenderError::Template(_) => defect(screen.template, error),
        })?;
    with_policy(
        sofie_engine::with_cleared_notice(html),
        screen,
        &new_nonce()?,
    )
}

fn page(
    screen: &ScreenPage<'_>,
    data: &dyn Show,
    notice: &[String],
    submitted: Option<&Submitted<'_>>,
) -> Result<Response> {
    let template = screen.rendering();
    let body = match submitted {
        Some(submitted) => template.render_submitted(data, submitted),
        None => template.render(data),
    }
    .map_err(|error| defect(screen.template, error))?;
    let nonce = new_nonce()?;
    let html = Page {
        title: screen.title,
        lang: screen.language.tag(),
        body: &body,
        notice,
        nonce: &nonce,
        #[cfg(feature = "islands")]
        islands: screen.islands.as_ref(),
        #[cfg(not(feature = "islands"))]
        islands: None,
        script_notices: screen.language.script_notices(),
    }
    .render();
    with_policy(html, screen, &nonce)
}

/// Typed data is read where it lies; data that is only `Serialize` is read from its JSON.
fn read<T>(
    screen: &ScreenPage<'_>,
    reply: Result<Reply>,
    render: impl FnOnce(&dyn Show) -> Result<T>,
) -> Result<T> {
    let reply = reply?;
    let key = screen.template.name();
    let _rendering = tracing::info_span!("render", otel.name = key, screen = key).entered();
    match reply.shown() {
        Some(_) => render(&reply),
        None => render(&reply.to_json()?),
    }
}

fn defect(template: &Template, error: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::internal(format!("rendering screen {}", template.name()), error)
}

/// A nonce CSP accepts as it is.
pub(crate) fn new_nonce() -> Result<String> {
    let random = u128::from_be_bytes(*Uuid::new_v4()?.as_bytes());
    Ok(format!("{random:032x}"))
}

pub(crate) fn with_policy(html: String, screen: &ScreenPage<'_>, nonce: &str) -> Result<Response> {
    let policy = screen.content_security.header(nonce)?;
    let mut response = answer::response(StatusCode::OK, Some(HTML), html);
    response
        .headers_mut()
        .insert(CONTENT_SECURITY_POLICY, policy);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_is_answered_when_asked_for() {
        assert_eq!(wanted(Caller::Json), Wanted::Json);
        assert_eq!(wanted(Caller::Browser), Wanted::Html { fragment: None });
    }

    #[test]
    fn a_fragment_needs_an_htmx_request_with_a_target() {
        let htmx = Caller::Htmx {
            target: Some("orders"),
            trigger: None,
        };
        assert_eq!(
            wanted(htmx),
            Wanted::Html {
                fragment: Some("orders")
            }
        );
        let untargeted = Caller::Htmx {
            target: None,
            trigger: None,
        };
        assert_eq!(wanted(untargeted), Wanted::Html { fragment: None });
    }
}
