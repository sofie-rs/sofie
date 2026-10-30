use sofie_core::{Error, Result};
use sofie_flags::{Feature, Features, Flags, Rule, Subject};

use super::Gates;
use crate::answer::Response;
use crate::caller::Caller;
use crate::routes::Route;
use crate::session::RequestSession;
use crate::target::Target;
use crate::{Area, RequestContext};
use sofie_db::Resolve;

pub struct RouteFeature {
    pub method: &'static str,
    pub path: &'static str,
    pub feature: Feature,
}

#[derive(Default)]
pub(super) struct FlagGate {
    flags: Option<Flags>,
    gated: &'static [RouteFeature],
}

impl Gates {
    #[must_use]
    pub fn features(mut self, flags: Flags) -> Self {
        self.flags.flags = Some(flags);
        self
    }

    /// Each is not found while its feature is off, as `.feature(…)` makes it.
    #[must_use]
    pub fn route_features(mut self, gated: &'static [RouteFeature]) -> Self {
        self.flags.gated = gated;
        self
    }
}

impl FlagGate {
    pub(super) fn held(&self) -> Option<Flags> {
        self.flags.clone()
    }

    pub(super) fn prepare(&self, routes: &mut [Route]) -> Result<()> {
        for gate in self.gated {
            let route = routes
                .iter_mut()
                .find(|route| route.method.as_str() == gate.method && route.path == gate.path)
                .ok_or_else(|| {
                    Error::internal_message(format!(
                        "the wiring gates route {} {}, which no module routes; run `sofie gen`",
                        gate.method, gate.path
                    ))
                })?;
            route.rules.feature = Some(gate.feature);
        }
        let flagged = routes.iter().find(|route| route.rules.feature.is_some());
        if let Some(route) = flagged.filter(|_| self.flags.is_none()) {
            return Err(no_flags(&format!(
                "route {} {} has a feature flag",
                route.method, route.path
            )));
        }
        Ok(())
    }

    /// As a caller without a session sees it, which is all a CORS preflight carries.
    #[cfg(feature = "cors")]
    pub(super) fn hides_from_anyone(&self, target: &Target) -> bool {
        let (Some(flags), Some(feature)) = (&self.flags, target.admission.rules.feature) else {
            return false;
        };
        let anyone = Subject {
            area: Some(target.audience.name),
            principal: None,
        };
        !flags.evaluate(anyone).enabled(feature)
    }

    /// A route whose flag is off is not found, so a hidden feature is not revealed.
    pub(super) fn open(
        &self,
        target: &Target,
        session: &RequestSession,
        caller: Caller<'_>,
    ) -> Result<Option<Features>, Box<Response>> {
        let Some(flags) = &self.flags else {
            return Ok(None);
        };
        let area = target.audience.name;
        let features = flags.evaluate(Subject {
            area: Some(area),
            principal: session.principal_in(area),
        });
        #[cfg(all(feature = "toolbar", debug_assertions))]
        let features = features.overridden(std::sync::Arc::clone(&session.toolbar.flags));
        let evaluated = features.clone();
        sofie_telemetry::attach_feature_flags(&tracing::Span::current(), move || {
            evaluated.evaluated()
        });
        if let Some(feature) = target.admission.rules.feature
            && !features.enabled(feature)
        {
            tracing::info!(
                feature_flag.key = feature.name(),
                http.route = target.pattern.as_str(),
                "not found: the route's feature is off"
            );
            return Err(Box::new(crate::answer::failure(Error::NotFound, caller)));
        }
        Ok(Some(features))
    }
}

pub(super) fn record(features: Option<&Features>) {
    let evaluated = features.map(Features::evaluated).unwrap_or_default();
    if !evaluated.is_empty() {
        tracing::Span::current().record("feature_flags", evaluated);
    }
}

impl<A: Area, M> Resolve<RequestContext<A, M>> for Features {
    fn resolve(context: &mut RequestContext<A, M>) -> impl Future<Output = Result<Self>> + Send {
        let features = context
            .matched
            .gates
            .features
            .clone()
            .ok_or_else(|| no_flags("a handler asks for `Features`"));
        std::future::ready(features)
    }
}

fn flags<A: Area, M>(context: &RequestContext<A, M>) -> Result<Flags> {
    context
        .matched
        .gates
        .flags
        .clone()
        .ok_or_else(|| no_flags("a handler asks for `Flags`"))
}

fn no_flags(asking: &str) -> Error {
    Error::internal_message(format!(
        "{asking}, but the app has no flags; give `Gates::features` the app's `Flags`"
    ))
}

impl<A: Area> Resolve<RequestContext<A>> for Flags {
    fn resolve(context: &mut RequestContext<A>) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(flags(context))
    }
}

/// What a GET may take: setting a flag needs `Flags`, in a POST.
pub struct FlagRules(Flags);

impl FlagRules {
    #[must_use]
    pub fn rules(&self) -> Vec<(String, Rule)> {
        self.0.rules()
    }
}

impl<A: Area, M> Resolve<RequestContext<A, M>> for FlagRules {
    fn resolve(context: &mut RequestContext<A, M>) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(flags(context).map(Self))
    }
}
