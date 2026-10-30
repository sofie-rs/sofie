//! A client area's answers: the page shell holding the first screen's envelope, the envelope
//! alone for the area's runtime, and a refused form's errors in the request's language.

use http::StatusCode;
use serde::Serialize;
use serde_json::{Map, Value, json};
use sofie_core::{Error, FieldError, Result};
use sofie_db::Reply;
use sofie_engine::{ClientBundle, ClientLinks, ClientShell, ClientView};

use crate::answer::{self, JSON, Refusal, Response};
use crate::caller::Caller;
use crate::language::Language;
use crate::screen::{ScreenPage, new_nonce, with_policy};

#[derive(Serialize)]
struct Envelope<'a> {
    screen: &'a str,
    title: &'a str,
    language: &'a str,
    data: Value,
    instants: Value,
    view: ClientView,
    navigation: Value,
}

/// The loader's JSON is the screen's data as `application/json` answers it.
fn envelope<'a>(screen: &'a ScreenPage<'a>, reply: Result<Reply>) -> Result<Envelope<'a>> {
    let (mut data, typed) = match reply? {
        Reply::Json(data) => (data, None),
        typed => (typed.to_json()?, Some(typed)),
    };
    screen.withhold(&mut data);
    let rendering = screen.rendering();
    #[cfg(feature = "listing")]
    rendering.translate_listing_errors(&mut data);
    let instants = typed
        .as_ref()
        .and_then(Reply::shown)
        .map_or(Value::Null, |shown| {
            sofie_engine::instant_fields(shown, &data)
        });
    Ok(Envelope {
        screen: screen.template.name(),
        title: screen.title,
        language: screen.language.tag(),
        data,
        instants,
        view: rendering.client_view(),
        navigation: screen.navigation_entries(),
    })
}

fn serializing(error: serde_json::Error) -> Error {
    Error::internal("serializing a client screen's envelope", error)
}

pub(crate) fn enveloped(screen: &ScreenPage<'_>, reply: Result<Reply>) -> Response {
    let answered = envelope(screen, reply)
        .and_then(|envelope| serde_json::to_string(&envelope).map_err(serializing))
        .map(|body| answer::response(StatusCode::OK, Some(JSON), body));
    answer::or_failure(answered, Caller::Screen)
}

pub(crate) fn shell(
    screen: &ScreenPage<'_>,
    bundle: &ClientBundle,
    reply: Result<Reply>,
) -> Result<Response> {
    let envelope = serde_json::to_value(envelope(screen, reply)?).map_err(serializing)?;
    let nonce = new_nonce()?;
    let links = links(bundle);
    let html = ClientShell {
        title: screen.title,
        lang: screen.language.tag(),
        nonce: &nonce,
        envelope: &envelope,
        links: &links,
        #[cfg(feature = "islands")]
        islands: screen.islands.as_ref(),
        #[cfg(not(feature = "islands"))]
        islands: None,
    }
    .render();
    with_policy(html, screen, &nonce)
}

fn links(bundle: &ClientBundle) -> ClientLinks {
    #[cfg(all(feature = "development", debug_assertions))]
    if let Some(links) = crate::development::client_links(bundle.area) {
        return links;
    }
    bundle.links()
}

/// Field messages and the notice are translated as a refused page would show them; each error
/// keeps its key and arguments, as a 422 answers them.
pub(crate) fn refused(refusal: &Refusal, language: Language<'_>) -> Response {
    let errors: Vec<Value> = match refusal {
        Refusal::Fields(fields) => fields
            .iter()
            .map(|error| field_error(error, language))
            .collect(),
        Refusal::Message(..) => Vec::new(),
    };
    let body = json!({ "errors": errors, "notice": refusal.messages(language) });
    answer::response(refusal.status(), Some(JSON), body.to_string())
}

fn field_error(error: &FieldError, language: Language<'_>) -> Value {
    let arguments: Map<String, Value> = error
        .arguments
        .iter()
        .map(|(name, value)| (name.to_string(), Value::from(value.as_str())))
        .collect();
    json!({
        "field": error.field,
        "key": error.key,
        "arguments": arguments,
        "message": language.field_error(error),
    })
}

impl crate::Screens {
    /// The bundles the app's build script compiled, one per client area.
    pub fn clients(&mut self, bundles: Vec<ClientBundle>) -> &mut Self {
        self.clients = bundles;
        self
    }

    pub(crate) fn client(&self, area: &str) -> Option<&ClientBundle> {
        self.clients.iter().find(|bundle| bundle.area == area)
    }
}

impl crate::assets::Assets {
    pub(crate) fn with_clients(mut self, screens: &crate::Screens) -> Self {
        for bundle in &screens.clients {
            for asset in [bundle.script, bundle.stylesheet] {
                self.served.push((asset.path(), asset));
            }
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_form_s_errors_keep_their_key_and_arguments() {
        let error =
            FieldError::new("code", "validation.max_length", "must be at most 4").with("max", "4");

        let response = refused(&Refusal::Fields(vec![error]), Language::plain());

        let body: Value =
            serde_json::from_slice(&response.into_body().into_inner().unwrap()).unwrap();
        assert_eq!(
            body["errors"],
            json!([{
                "field": "code",
                "key": "validation.max_length",
                "arguments": { "max": "4" },
                "message": "must be at most 4",
            }])
        );
    }
}
