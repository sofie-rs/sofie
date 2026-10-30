use std::marker::PhantomData;

use http::Method;
use sofie_db::{Call, Reach};

use crate::area::AreaPrincipal;
use crate::handler::{Endpoint, Loader, endpoint, loader_endpoint};
use crate::session::RequestSession;
use crate::throttle::Rate;
use crate::{Access, Area, RequestContext};

pub(crate) const BODY_LIMIT: usize = 64 * 1024;

#[derive(Clone)]
pub(crate) enum View {
    Json,
    #[cfg(feature = "engine")]
    Screen(ScreenView),
    #[cfg(feature = "engine")]
    Refresh(Refresh),
    Redirect(String),
    /// To the `location` the action replied, a page elsewhere such as a payment provider's.
    Follow,
    /// To the local `location` the action replied, else as `Redirect`.
    RedirectOrFollow(String),
    Document(Media),
}

/// What a document route's text is, for crawlers and feed readers rather than pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Media {
    Xml,
    Text,
    /// From a list of records the loader answers, not from `Text`.
    #[cfg(feature = "csv")]
    Csv,
}

/// What a document route's loader answers: its whole body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text(pub String);

impl sofie_db::Outcome for Text {
    fn reply(self) -> sofie_core::Result<sofie_db::Reply> {
        Ok(sofie_db::Reply::Json(serde_json::Value::String(self.0)))
    }
}

impl sofie_db::ReadOnly for Text {}

impl Media {
    pub(crate) fn content_type(self) -> &'static str {
        match self {
            Self::Xml => "application/xml; charset=utf-8",
            Self::Text => "text/plain; charset=utf-8",
            #[cfg(feature = "csv")]
            Self::Csv => "text/csv; charset=utf-8",
        }
    }
}

#[cfg(feature = "engine")]
#[derive(Clone)]
pub(crate) struct ScreenView {
    pub(crate) key: String,
    pub(crate) title: String,
    #[cfg(feature = "resources")]
    pub(crate) builtin: Option<std::sync::Arc<crate::gate::optional::resource::Builtin>>,
}

#[cfg(feature = "engine")]
#[derive(Clone)]
pub(crate) struct Refresh {
    pub(crate) screen: String,
    pub(crate) fragment: String,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Audience {
    pub(crate) name: &'static str,
    /// Sessions are sealed for an area's name, so two area types of one name would share them.
    pub(crate) area: std::any::TypeId,
    pub(crate) access: Access,
    pub(crate) sign_in: &'static str,
    #[cfg(feature = "tokens")]
    pub(crate) tokens: bool,
}

impl Audience {
    fn of<A: Area>() -> Self {
        Self {
            name: A::NAME,
            area: std::any::TypeId::of::<A>(),
            access: <A::Principal as AreaPrincipal>::ACCESS,
            sign_in: A::SIGN_IN,
            #[cfg(feature = "tokens")]
            tokens: A::TOKENS,
        }
    }

    /// `None` turns the request away: an owned area without a principal.
    pub(crate) fn reach(self, session: &RequestSession) -> Option<Reach> {
        match self.access {
            Access::Public => Some(Reach::All),
            Access::Owned => session.principal_in(self.name).map(Reach::OwnedBy),
            #[cfg(feature = "permissions")]
            Access::Permitted => session.principal_in(self.name).map(Reach::AllAs),
        }
    }
}

#[cfg(feature = "engine")]
#[must_use]
pub fn screen_key(module: &str, area: &str, name: &str) -> String {
    format!("{module}/{area}/{name}")
}

pub(crate) struct Route {
    pub(crate) module: &'static str,
    pub(crate) name: Option<&'static str>,
    pub(crate) method: Method,
    pub(crate) path: String,
    pub(crate) audience: Audience,
    pub(crate) endpoint: Endpoint,
    pub(crate) view: View,
    pub(crate) rules: RouteRules,
}

impl Route {
    /// Leaked once per route as the app is built, so spans and metrics label it without copying.
    pub(crate) fn name_as(&mut self, area: &str, handler: &str) {
        let name = format!("{}.{area}.{handler}", self.module);
        self.name = Some(Box::leak(name.into_boxed_str()));
    }
}

#[derive(Clone, Default)]
pub(crate) struct RouteRules {
    pub(crate) throttle: Option<Rate>,
    pub(crate) body_limit: Option<usize>,
    #[cfg(feature = "uploads")]
    pub(crate) file_limit: Option<usize>,
    #[cfg(feature = "signed-links")]
    pub(crate) signed: bool,
    #[cfg(feature = "idempotency")]
    pub(crate) idempotent: bool,
    #[cfg(feature = "webhooks")]
    pub(crate) verify: Option<std::sync::Arc<dyn crate::webhook::Verify>>,
    #[cfg(feature = "flags")]
    pub(crate) feature: Option<sofie_flags::Feature>,
    #[cfg(feature = "entitlements")]
    pub(crate) entitlement: Option<crate::Entitlement>,
    /// Overrides the permission the wiring derives.
    #[cfg(feature = "permissions")]
    pub(crate) permission: Option<crate::Permission>,
    #[cfg(feature = "live")]
    pub(crate) live: Vec<String>,
    #[cfg(feature = "screen-cache")]
    pub(crate) cached: Option<std::time::Duration>,
    /// Queues a `#[job]` instead of running it, which an owned area cannot.
    #[cfg(feature = "jobs")]
    pub(crate) queues: bool,
}

/// Handlers are plain `async fn`s whose arguments all resolve in `A`; a GET's in its `ReadContext`.
pub struct Routes<A: Area> {
    pub(crate) module: &'static str,
    pub(crate) added: Vec<Route>,
    #[cfg(feature = "islands")]
    pub(crate) islands: Vec<crate::islands::IslandScreen>,
    area: PhantomData<fn() -> A>,
}

pub struct PageRoute<'r, A: Area> {
    route: &'r mut Route,
    area: PhantomData<fn() -> A>,
}

pub struct ActionRoute<'r, A: Area> {
    route: &'r mut Route,
    area: PhantomData<fn() -> A>,
}

macro_rules! modifiers {
    ($($builder:ident),*) => {$(
        impl<'r, A: Area> $builder<'r, A> {
            pub(crate) fn new(route: &'r mut Route) -> Self {
                Self {
                    route,
                    area: PhantomData,
                }
            }

            /// For a library's route: named `module.area.handler` like those `sofie gen` names.
            pub fn name(&mut self, handler: &str) -> &mut Self {
                self.route.name_as(A::NAME, handler);
                self
            }

            /// For a library's route serving `Member`'s members from this area, as sign-in serves each
            /// member area from `Web`.
            pub fn name_for<Member: Area>(&mut self, handler: &str) -> &mut Self {
                self.route.name_as(Member::NAME, handler);
                self
            }

            /// Per principal, or per client address without one.
            pub fn throttle(&mut self, rate: Rate) -> &mut Self {
                self.rule(|rules| rules.throttle = Some(rate))
            }

            #[cfg(feature = "signed-links")]
            pub fn signed(&mut self) -> &mut Self {
                self.rule(|rules| rules.signed = true)
            }

            /// Not found while `feature` is off, so a hidden feature is not revealed.
            #[cfg(feature = "flags")]
            pub fn feature(&mut self, feature: sofie_flags::Feature) -> &mut Self {
                self.rule(|rules| rules.feature = Some(feature))
            }

            /// Refused with 403 to a principal whose plan does not hold `entitlement`.
            #[cfg(feature = "entitlements")]
            pub fn entitled(&mut self, entitlement: crate::Entitlement) -> &mut Self {
                self.rule(|rules| rules.entitlement = Some(entitlement))
            }

            pub(crate) fn rule(&mut self, change: impl FnOnce(&mut RouteRules)) -> &mut Self {
                change(&mut self.route.rules);
                self
            }
        }
    )*};
}

modifiers!(PageRoute, ActionRoute);

macro_rules! actions {
    ($($name:ident: $method:ident),*) => {$(
        pub fn $name<Args: 'static, H>(&mut self, path: &str, handler: H) -> ActionRoute<'_, A>
        where
            H: Call<RequestContext<A>, Args, Outcome: Send>,
        {
            let endpoint = endpoint(handler);
            ActionRoute::new(self.push(Method::$method, path, endpoint, View::Json, RouteRules::default()))
        }
    )*};
}

impl<A: Area> Routes<A> {
    pub(crate) fn new(module: &'static str) -> Self {
        Self {
            module,
            added: Vec::new(),
            #[cfg(feature = "islands")]
            islands: Vec::new(),
            area: PhantomData,
        }
    }

    actions!(post: POST, put: PUT, patch: PATCH, delete: DELETE);

    /// Also answers HEAD.
    pub fn get<Args: 'static, H: Loader<A, Args>>(
        &mut self,
        path: &str,
        loader: H,
    ) -> PageRoute<'_, A> {
        let endpoint = loader_endpoint(loader);
        PageRoute::new(self.push(
            Method::GET,
            path,
            endpoint,
            View::Json,
            RouteRules::default(),
        ))
    }

    #[cfg(feature = "engine")]
    pub fn screen<Args: 'static, H: Loader<A, Args>>(
        &mut self,
        path: &str,
        name: &str,
        loader: H,
    ) -> PageRoute<'_, A> {
        let view = View::Screen(ScreenView {
            key: screen_key(self.module, A::NAME, name),
            title: sofie_engine::humanize(name),
            #[cfg(feature = "resources")]
            builtin: None,
        });
        let endpoint = loader_endpoint(loader);
        PageRoute::new(self.push(Method::GET, path, endpoint, view, RouteRules::default()))
    }

    pub(crate) fn push(
        &mut self,
        method: Method,
        path: &str,
        endpoint: Endpoint,
        view: View,
        rules: RouteRules,
    ) -> &mut Route {
        self.added.push_mut(Route {
            module: self.module,
            name: None,
            method,
            path: format!("{}{path}", A::PREFIX),
            audience: Audience::of::<A>(),
            endpoint,
            view,
            rules,
        })
    }
}

impl<A: Area> PageRoute<'_, A> {
    /// Answers the loader's [`Text`] as `media` instead of JSON, such as a `/sitemap.xml`.
    pub fn document(&mut self, media: Media) -> &mut Self {
        self.route.view = View::Document(media);
        self
    }
}

impl<A: Area> ActionRoute<'_, A> {
    pub fn body_limit(&mut self, bytes: usize) -> &mut Self {
        self.rule(|rules| rules.body_limit = Some(bytes))
    }

    /// Each file of a form; the whole body stays within `body_limit`, by default one file and
    /// the text fields.
    #[cfg(feature = "uploads")]
    pub fn file_limit(&mut self, bytes: usize) -> &mut Self {
        self.rule(|rules| rules.file_limit = Some(bytes))
    }

    /// A retry gets the first request's answer; the action's effects are written once.
    #[cfg(feature = "idempotency")]
    pub fn idempotent(&mut self) -> &mut Self {
        self.rule(|rules| rules.idempotent = true)
    }

    /// JSON callers still get the action's reply.
    #[cfg(feature = "engine")]
    pub fn then_screen(&mut self, name: &str, fragment: &str) {
        let refresh = Refresh {
            screen: screen_key(self.route.module, A::NAME, name),
            fragment: fragment.to_owned(),
        };
        self.route.view = View::Refresh(refresh);
    }

    /// JSON callers still get the action's reply. Each `{name}` in `location` is the route's
    /// path parameter of that name, or else the reply's field: `"/orders/{id}"` after an action
    /// replying the order it created, `"/shops/{shop}/orders"` after `/shops/{shop}/orders/new`.
    pub fn then_redirect(&mut self, location: &str) {
        self.route.view = View::Redirect(location.to_owned());
    }

    /// A browser goes to the `https://` `location` the action replies; JSON callers get the reply.
    pub fn then_follow(&mut self) {
        self.route.view = View::Follow;
    }

    /// As `then_redirect`, `next` included, unless the action replies a `location` of this
    /// app, such as a second step of signing in, which the browser then goes to instead.
    pub fn then_redirect_or_follow(&mut self, location: &str) {
        self.route.view = View::RedirectOrFollow(location.to_owned());
    }
}
