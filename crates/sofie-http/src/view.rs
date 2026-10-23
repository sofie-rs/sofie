use sofie_core::{Error, Result};
use sofie_db::{Db, Reply};

use crate::answer::{self, Redirect, Response};
use crate::caller::Caller;
use crate::context::Matched;
use crate::guard::signed_in_destination;
use crate::pattern;
use crate::pipeline::Asked;
use crate::router::Router;
#[cfg(feature = "engine")]
use crate::routes::ScreenView;
use crate::routes::View;
#[cfg(feature = "engine")]
use crate::screen::{self, ScreenPage};
use crate::session::RequestSession;
#[cfg(feature = "engine")]
use crate::target::ScreenRoute;
use crate::target::Target;
#[cfg(feature = "engine")]
use crate::urlencoded::Encoding;
#[cfg(feature = "engine")]
use crate::{Screens, form};

pub(crate) struct RouteAnswer<'r> {
    pub(crate) db: &'r Db,
    pub(crate) router: &'r Router,
    #[cfg(feature = "engine")]
    pub(crate) screens: &'r Screens,
    pub(crate) target: &'r Target,
    pub(crate) asked: Asked<'r>,
    #[cfg(all(feature = "gates", feature = "engine"))]
    pub(crate) gates: &'r crate::gate::RequestGates,
    #[cfg(feature = "screen-cache")]
    pub(crate) screen_cache: &'r crate::screen_cache::ScreenCache,
    #[cfg(feature = "live")]
    pub(crate) keys: &'r sofie_auth::SigningKeys,
    #[cfg(feature = "live")]
    pub(crate) principal: Option<sofie_core::Uuid>,
}

impl RouteAnswer<'_> {
    pub(crate) async fn answer(&self, matched: Matched) -> Response {
        match &self.target.view {
            View::Json | View::Document(_) => self.loaded(matched).await,
            #[cfg(feature = "engine")]
            View::Screen(_) => self.loaded(matched).await,
            _ => self.after_action(matched).await,
        }
    }

    async fn loaded(&self, matched: Matched) -> Response {
        #[cfg(feature = "screen-cache")]
        if let Some(time) = self.target.admission.rules.cached {
            let (result, topics) = self
                .screen_cache
                .read(self.db, self.target, time, matched, &self.asked.request.uri)
                .await;
            #[cfg(feature = "live")]
            let heard = (!self.target.admission.rules.live.is_empty()).then_some(topics);
            #[cfg(not(feature = "live"))]
            let _ = topics;
            return self
                .shown(
                    result,
                    #[cfg(feature = "live")]
                    heard,
                )
                .await;
        }
        let load = (self.target.endpoint)(self.db.clone(), matched);
        #[cfg(feature = "live")]
        let (result, heard) = crate::live::heard(&self.target.admission.rules, load).await;
        #[cfg(not(feature = "live"))]
        let result = load.await;
        self.shown(
            result,
            #[cfg(feature = "live")]
            heard,
        )
        .await
    }

    #[cfg_attr(
        not(feature = "live"),
        expect(
            clippy::unused_async_trait_impl,
            reason = "only marking a live screen waits"
        )
    )]
    async fn shown(
        &self,
        result: Result<Reply>,
        #[cfg(feature = "live")] heard: Option<sofie_db::Topics>,
    ) -> Response {
        let caller = self.asked.caller;
        if let Err(Error::Moved { from, to }) = &result {
            return answer::moved(&self.asked.request.uri, from, to, caller);
        }
        #[cfg(feature = "etag")]
        if let Some(unchanged) = crate::etag::unchanged(self.asked, &result) {
            return unchanged;
        }
        #[cfg(feature = "engine")]
        if let View::Screen(view) = &self.target.view {
            let response = self.on_screen(view, |page| screen::answer(result, caller, page));
            #[cfg(feature = "live")]
            let response = crate::live::marked(
                response,
                heard,
                self.target,
                caller,
                self.keys,
                self.principal,
                self.db.clock().now().timestamp().as_second(),
            )
            .await;
            return response;
        }
        #[cfg(feature = "chosen-screens")]
        let result = match crate::chosen::answer(self, result) {
            Ok(chosen) => return chosen,
            Err(result) => result,
        };
        if let View::Document(media) = self.target.view {
            return answer::document(result, media, caller);
        }
        #[cfg(feature = "engine")]
        let result = answer::in_language(result, self.screens.language(self.asked.request));
        answer::outcome(result, caller)
    }

    async fn after_action(&self, matched: Matched) -> Response {
        let caller = self.asked.caller;
        let action = &self.target.endpoint;
        if !takes_views(caller) {
            let result = action(self.db.clone(), matched).await;
            #[cfg(feature = "engine")]
            let result = answer::in_language(result, self.screens.language(self.asked.request));
            return answer::outcome(result, caller);
        }
        #[cfg(feature = "engine")]
        let (again, posted) = (
            matched.for_loader(),
            typed(
                matched.body.as_deref().unwrap_or_default(),
                matched.encoding,
            ),
        );
        let session = matched.session.clone();
        let parameters = match &self.target.view {
            View::Redirect(location) | View::RedirectOrFollow(location)
                if location.contains('{') =>
            {
                matched.parameters.clone()
            }
            _ => Vec::new(),
        };
        let reply = match action(self.db.clone(), matched).await {
            Ok(reply) => reply,
            Err(error) => {
                #[cfg(feature = "clients")]
                if caller == Caller::Screen {
                    let language = self.screens.language(self.asked.request);
                    return crate::client_page::refused(&answer::Refusal::of(error), language);
                }
                #[cfg(feature = "engine")]
                if let (Caller::FormPost, Some(page)) = (
                    caller,
                    self.target.form_page.as_ref().or(self.target.page.as_ref()),
                ) {
                    return self.refused_page(error, page, again, &posted).await;
                }
                #[cfg(feature = "engine")]
                if let Caller::Htmx { trigger, .. } = caller {
                    return self.refused_notice(error, trigger);
                }
                return answer::failure(error, caller);
            }
        };
        match &self.target.view {
            View::Redirect(location) => match filled(location, &parameters, &reply) {
                Ok(location) => self.redirected(&location, &session),
                Err(error) => answer::failure(error, caller),
            },
            View::RedirectOrFollow(location) => match replied_location(&reply) {
                Ok(Some(replied)) if crate::guard::is_local_path(&replied) => {
                    self.redirected_to(&replied)
                }
                Ok(_) => match filled(location, &parameters, &reply) {
                    Ok(location) => self.redirected(&location, &session),
                    Err(error) => answer::failure(error, caller),
                },
                Err(error) => answer::failure(error, caller),
            },
            View::Follow => match replied_location(&reply) {
                Ok(Some(replied)) if is_followed(&replied) => self.redirected_to(&replied),
                Ok(_) => answer::failure(
                    Error::internal_message(
                        "a route that follows its reply replied no `https://` or local `location`",
                    ),
                    caller,
                ),
                Err(error) => answer::failure(error, caller),
            },
            #[cfg(feature = "engine")]
            View::Refresh(refresh) => self.refreshed(&refresh.fragment, again, reply).await,
            _ => answer::outcome(Ok(reply), caller),
        }
    }

    fn redirected(&self, location: &str, session: &RequestSession) -> Response {
        let location = signed_in_destination(self.asked.request, session, self.router)
            .unwrap_or_else(|| location.to_owned());
        self.redirected_to(&location)
    }

    fn redirected_to(&self, location: &str) -> Response {
        let caller = self.asked.caller;
        #[cfg(feature = "clients")]
        if caller == Caller::Screen && !crate::guard::is_local_path(location) {
            return external_follow(location);
        }
        let kind = match caller {
            Caller::FormPost => Redirect::SeeOther,
            #[cfg(feature = "clients")]
            Caller::Screen => Redirect::SeeOther,
            _ => Redirect::Navigate,
        };
        answer::or_failure(answer::redirect(kind, location), caller)
    }
}

#[cfg(feature = "engine")]
impl RouteAnswer<'_> {
    async fn refreshed(&self, fragment: &str, again: Matched, reply: Reply) -> Response {
        let caller = self.asked.caller;
        let Some(page) = &self.target.page else {
            let error = Error::internal_message("a refresh without its page passed boot");
            return answer::failure(error, caller);
        };
        if caller == Caller::FormPost {
            return answer::or_failure(redirect_to(page, &again.parameters), caller);
        }
        if !self.admits(page) {
            return answer::outcome(Ok(reply), caller);
        }
        let loaded = (page.loader)(self.db.clone(), again).await;
        #[cfg(feature = "clients")]
        if caller == Caller::Screen {
            return self.on_screen(&page.screen, |screen| {
                crate::client_page::enveloped(screen, loaded)
            });
        }
        self.on_screen(&page.screen, |screen| {
            screen::refreshed(loaded, caller, screen, fragment)
        })
    }

    async fn refused_page(
        &self,
        error: Error,
        page: &ScreenRoute,
        again: Matched,
        input: &serde_json::Value,
    ) -> Response {
        if !self.admits(page) {
            return answer::failure(error, self.asked.caller);
        }
        let refusal = answer::Refusal::of(error);
        let loaded = (page.loader)(self.db.clone(), again).await;
        self.on_screen(&page.screen, |screen| {
            screen::refused_page(loaded, &refusal, self.asked, screen, input)
        })
    }

    #[cfg(feature = "engine")]
    fn refused_notice(&self, error: Error, trigger: Option<&str>) -> Response {
        let language = self.screens.language(self.asked.request);
        let page = self.target.form_page.as_ref().or(self.target.page.as_ref());
        let template = match page.map(|page| self.screens.template(&page.screen.key)) {
            Some(Err(missing)) => return answer::failure(missing, self.asked.caller),
            Some(Ok(template)) => Some(template),
            None => None,
        };
        let shown = template
            .as_deref()
            .zip(trigger)
            .and_then(|(template, form)| sofie_engine::form_fields(template, form));
        answer::Refusal::of(error).notice(trigger, shown.as_deref(), language)
    }

    #[cfg_attr(
        not(feature = "gates"),
        expect(clippy::unused_self, reason = "only gates refuse a page")
    )]
    fn admits(&self, page: &ScreenRoute) -> bool {
        #[cfg(feature = "gates")]
        return self.gates.admit(page);
        #[cfg(not(feature = "gates"))]
        {
            let _ = page;
            true
        }
    }

    pub(crate) fn on_screen(
        &self,
        view: &ScreenView,
        answer: impl FnOnce(&ScreenPage<'_>) -> Response,
    ) -> Response {
        let template = match self.screens.template(&view.key) {
            Ok(template) => template,
            Err(error) => return answer::failure(error, self.asked.caller),
        };
        answer(&ScreenPage {
            template: &template,
            title: &view.title,
            language: self.screens.language(self.asked.request),
            content_security: &self.target.admission.area.content_security,
            #[cfg(feature = "islands")]
            islands: self.screens.islands.links(self.target.audience.name),
            #[cfg(feature = "clients")]
            client: self.screens.client(self.target.audience.name),
            #[cfg(feature = "gates")]
            answer: self,
        })
    }
}

/// A client area's runtime posts like a form, and is answered by the route's view too.
fn takes_views(caller: Caller<'_>) -> bool {
    #[cfg(feature = "clients")]
    if caller == Caller::Screen {
        return true;
    }
    matches!(caller, Caller::Htmx { .. } | Caller::FormPost)
}

/// Each `{name}` of a redirect is the route's path parameter of that name, or else the reply's
/// field, such as the `id` of the record an action created.
fn filled(location: &str, parameters: &[(String, String)], reply: &Reply) -> Result<String> {
    if !location.contains('{') {
        return Ok(location.to_owned());
    }
    let json = match reply {
        Reply::Json(json) => Some(std::borrow::Cow::Borrowed(json)),
        Reply::Data(_) => Some(std::borrow::Cow::Owned(reply.to_json()?)),
        Reply::Empty | Reply::List(_) => None,
    };
    let mut values = parameters.to_vec();
    for name in pattern::parameter_names(location) {
        if values.iter().any(|(given, _)| given == name) {
            continue;
        }
        let replied = json.as_deref().and_then(|json| match &json[name] {
            serde_json::Value::String(text) => Some(text.clone()),
            serde_json::Value::Number(number) => Some(number.to_string()),
            _ => None,
        });
        let Some(replied) = replied else {
            return Err(Error::internal_message(format!(
                "redirecting to `{location}`: `{{{name}}}` is neither a parameter of the route's \
                 path nor a text or number field of the action's reply"
            )));
        };
        values.push((name.to_owned(), replied));
    }
    pattern::fill(location, &values)
        .filter(|filled| crate::guard::is_local_path(filled))
        .ok_or_else(|| Error::internal_message(format!("filling the redirect `{location}`")))
}

fn replied_location(reply: &Reply) -> Result<Option<String>> {
    let location = |json: &serde_json::Value| json["location"].as_str().map(str::to_owned);
    Ok(match reply {
        Reply::Json(json) => location(json),
        Reply::Data(_) => location(&reply.to_json()?),
        Reply::Empty | Reply::List(_) => None,
    })
}

/// The runtime's fetch would follow a 303 to another site, which sends it no CORS headers, so
/// the runtime is named the address and goes there itself.
#[cfg(feature = "clients")]
fn external_follow(location: &str) -> Response {
    let body = serde_json::json!({ "location": location }).to_string();
    answer::response(http::StatusCode::OK, Some(answer::JSON), body)
}

/// Only `https://` or a path of this app, so an action echoing what a visitor sent cannot
/// send a browser to a script.
fn is_followed(location: &str) -> bool {
    location.starts_with("https://") || crate::guard::is_local_path(location)
}

/// Cannot fail: the boot check ensures the action has every parameter of the screen's path.
#[cfg(feature = "engine")]
fn redirect_to(page: &ScreenRoute, parameters: &[(String, String)]) -> Result<Response> {
    let location = pattern::fill(&page.path, parameters)
        .ok_or_else(|| Error::internal_message(format!("filling the screen path {}", page.path)))?;
    answer::redirect(Redirect::SeeOther, &location)
}

/// A multipart form arrives as the JSON of its fields; a body that is no form was refused as
/// such and shows nothing typed. A file comes back to no one: its input cannot be refilled.
#[cfg(feature = "engine")]
fn typed(posted: &[u8], encoding: Encoding) -> serde_json::Value {
    let fields = match encoding {
        Encoding::Form => form::parse(posted).ok(),
        Encoding::Json => serde_json::from_slice(posted).ok(),
    };
    #[cfg_attr(not(feature = "uploads"), expect(unused_mut))]
    let mut fields: serde_json::Map<String, serde_json::Value> = fields.unwrap_or_default();
    #[cfg(feature = "uploads")]
    fields.retain(|_, value| !crate::uploads::is_upload(value));
    serde_json::Value::Object(fields)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_reply_names_its_location_as_json_does() {
        let typed = Reply::data(std::collections::BTreeMap::from([(
            "location",
            "/orders/1",
        )]));
        let json = Reply::Json(serde_json::json!({ "location": "/orders/1" }));

        for reply in [typed, json] {
            assert_eq!(
                replied_location(&reply).unwrap().as_deref(),
                Some("/orders/1")
            );
        }
        assert_eq!(replied_location(&Reply::Empty).unwrap(), None);
    }
}
