use std::collections::HashMap;
use std::sync::Arc;

#[cfg(feature = "engine")]
use http::Method;
use sofie_core::registrations::{Key, Problem, validate};
use sofie_core::{Error, Result};

use crate::admission::{Admission, AreaPolicy};
use crate::guard::is_local_path;
use crate::handler::Endpoint;
use crate::router::Router;
use crate::routes::{Audience, Route, View};
use crate::{Access, RouteName, health};
#[cfg(feature = "engine")]
use crate::{Screens, pattern, routes::ScreenView};

pub(crate) struct Target {
    pub(crate) endpoint: Endpoint,
    pub(crate) view: View,
    /// Where a form posted without JavaScript comes back to.
    #[cfg(feature = "engine")]
    pub(crate) page: Option<ScreenRoute>,
    /// The screen at the action's own path, holding its form: a refused form comes back there.
    #[cfg(feature = "engine")]
    pub(crate) form_page: Option<ScreenRoute>,
    /// Telemetry names a request by its pattern, never the path with its ids.
    pub(crate) pattern: String,
    pub(crate) name: Option<&'static str>,
    pub(crate) module: &'static str,
    pub(crate) audience: Audience,
    pub(crate) admission: Admission,
}

#[cfg(feature = "engine")]
pub(crate) struct ScreenRoute {
    pub(crate) loader: Endpoint,
    pub(crate) screen: ScreenView,
    pub(crate) path: String,
    /// The page's own gates, which an action showing it passes too.
    #[cfg(feature = "flags")]
    pub(crate) feature: Option<sofie_flags::Feature>,
    #[cfg(feature = "permissions")]
    pub(crate) permission: Option<crate::Permission>,
    #[cfg(feature = "entitlements")]
    pub(crate) entitlement: Option<crate::Entitlement>,
}

pub(crate) fn route_table(
    routes: &[Route],
    areas: HashMap<&'static str, AreaPolicy>,
    names: &[RouteName],
    #[cfg(feature = "engine")] screens: &Screens,
) -> Result<Router> {
    let areas: HashMap<&str, Arc<AreaPolicy>> = areas
        .into_iter()
        .map(|(name, policy)| (name, Arc::new(policy)))
        .collect();
    let route_names: Vec<Option<&'static str>> = routes
        .iter()
        .map(|route| route_name(route, names))
        .collect();
    refuse_clashes(routes, &route_names)?;
    refuse_shared_area_names(routes)?;
    let mut table = Router::default();
    for (route, name) in routes.iter().zip(route_names) {
        health::refuse_reserved(&route.method, &route.path)?;
        let area = areas.get(route.audience.name).cloned().unwrap_or_default();
        if route.audience.access != Access::Public {
            require_local(route, route.audience.sign_in)?;
        }
        #[cfg(feature = "jobs")]
        require_unowned(route)?;
        if let View::Redirect(location) | View::RedirectOrFollow(location) = &route.view {
            require_local(route, location)?;
        }
        let target = Target {
            endpoint: Arc::clone(&route.endpoint),
            view: route.view.clone(),
            #[cfg(feature = "engine")]
            page: page_of(route, routes, screens)?,
            #[cfg(feature = "engine")]
            form_page: form_page_of(route, routes),
            pattern: route.path.clone(),
            name,
            module: route.module,
            audience: route.audience,
            admission: Admission::new(&route.rules, area),
        };
        table.insert(&route.method, &route.path, target)?;
    }
    Ok(table)
}

fn route_name(route: &Route, names: &[RouteName]) -> Option<&'static str> {
    route.name.or_else(|| {
        names
            .iter()
            .find(|name| route.method == name.method && name.path == route.path)
            .map(|name| name.name)
    })
}

fn refuse_shared_area_names(routes: &[Route]) -> Result<()> {
    let mut areas = HashMap::new();
    for audience in routes.iter().map(|route| route.audience) {
        let first = *areas.entry(audience.name).or_insert(audience.area);
        if first != audience.area {
            return Err(Error::internal_message(format!(
                "two area types are named `{}`, so a session of one would open the other; \
                 give each its own `NAME`",
                audience.name
            )));
        }
    }
    Ok(())
}

fn refuse_clashes(routes: &[Route], names: &[Option<&str>]) -> Result<()> {
    let keys: Vec<Key> = routes
        .iter()
        .zip(names)
        .map(|(route, name)| Key::Route {
            method: route.method.as_str(),
            path: Some(&route.path),
            name: *name,
        })
        .collect();
    let problems: Vec<String> = validate(&keys)
        .into_iter()
        .map(|problem| describe(problem, routes, names))
        .collect();
    if problems.is_empty() {
        return Ok(());
    }
    Err(Error::internal_message(problems.join("; ")))
}

fn describe(problem: Problem, routes: &[Route], names: &[Option<&str>]) -> String {
    let route = |index: usize| {
        let route = &routes[index];
        format!(
            "{} {} of module `{}`",
            route.method, route.path, route.module
        )
    };
    match problem {
        Problem::SamePath(first, second) => format!(
            "routes {} and {} are served at one path, and only one handler can answer it",
            route(first),
            route(second)
        ),
        Problem::SameRouteName(first, second) => format!(
            "routes {} and {} are both named `{}`; spans and route builders need one name per \
             route",
            route(first),
            route(second),
            names[first].unwrap_or_default()
        ),
        Problem::SameJobName(..) | Problem::Interval(..) => {
            unreachable!("a route table validates no jobs")
        }
    }
}

#[cfg(feature = "jobs")]
fn require_unowned(route: &Route) -> Result<()> {
    if !route.rules.queues || route.audience.access != Access::Owned {
        return Ok(());
    }
    Err(route_error(
        route,
        "queues a job, which reaches every record, from an area whose principals own their \
         records; run an action that checks the record and queues the job through `Jobs`",
    ))
}

fn require_local(route: &Route, path: &str) -> Result<()> {
    if is_local_path(path) {
        return Ok(());
    }
    Err(route_error(
        route,
        format_args!(
            "sends browsers to `{path}`, which is not a path of this app; write it as `/path`"
        ),
    ))
}

fn route_error(route: &Route, problem: impl std::fmt::Display) -> Error {
    Error::internal_message(format!("route {} {} {problem}", route.method, route.path))
}

#[cfg(feature = "engine")]
fn page_of(route: &Route, routes: &[Route], screens: &Screens) -> Result<Option<ScreenRoute>> {
    match &route.view {
        View::Json | View::Document(_) => Ok(None),
        View::Screen(screen) => require_loaded(route, &screen.key, screens).map(|()| None),
        View::Refresh(refresh) => {
            require_loaded(route, &refresh.screen, screens)?;
            let page = screen_route_of(route, &refresh.screen, routes)?;
            require_fillable(route, &page)?;
            Ok(Some(page))
        }
        View::Redirect(_) | View::Follow | View::RedirectOrFollow(_) => Ok(page_at(route, routes)),
    }
}

/// A redirecting action's page is already the one at its path.
#[cfg(feature = "engine")]
fn form_page_of(route: &Route, routes: &[Route]) -> Option<ScreenRoute> {
    match &route.view {
        View::Refresh(_) => page_at(route, routes),
        _ => None,
    }
}

#[cfg(feature = "engine")]
fn require_loaded(route: &Route, key: &str, screens: &Screens) -> Result<()> {
    if screens.contains(key) {
        return Ok(());
    }
    Err(route_error(
        route,
        format_args!("renders screen {key}, which is not loaded"),
    ))
}

#[cfg(feature = "engine")]
fn screen_route(route: &Route, screen: &ScreenView) -> ScreenRoute {
    ScreenRoute {
        loader: Arc::clone(&route.endpoint),
        screen: screen.clone(),
        path: route.path.clone(),
        #[cfg(feature = "flags")]
        feature: route.rules.feature,
        #[cfg(feature = "permissions")]
        permission: route
            .rules
            .permission
            .filter(|_| route.audience.access == Access::Permitted),
        #[cfg(feature = "entitlements")]
        entitlement: route.rules.entitlement,
    }
}

/// A page whose route admits fewer callers than the action, or reads with another area's
/// reach, is not the action's to show; its own gates are checked when it is shown.
#[cfg(feature = "engine")]
fn may_show(action: &Route, page: &Route) -> bool {
    #[cfg(feature = "signed-links")]
    if page.rules.signed && !action.rules.signed {
        return false;
    }
    page.audience.name == action.audience.name
}

#[cfg(feature = "engine")]
fn page_at(route: &Route, routes: &[Route]) -> Option<ScreenRoute> {
    routes.iter().find_map(|candidate| match &candidate.view {
        View::Screen(screen)
            if candidate.method == Method::GET
                && candidate.path == route.path
                && may_show(route, candidate) =>
        {
            Some(screen_route(candidate, screen))
        }
        _ => None,
    })
}

#[cfg(feature = "engine")]
/// Exactly one, or the refresh could not tell which loader's data to show.
fn screen_route_of(route: &Route, key: &str, routes: &[Route]) -> Result<ScreenRoute> {
    let mut screen_routes = routes.iter().filter_map(|candidate| match &candidate.view {
        View::Screen(screen) if screen.key == key => Some((candidate, screen)),
        _ => None,
    });
    match (screen_routes.next(), screen_routes.next()) {
        (Some((page, screen)), None) if may_show(route, page) => Ok(screen_route(page, screen)),
        (Some(_), None) => Err(route_error(
            route,
            format_args!(
                "refreshes screen {key}, whose route is signed or in another area, which this \
                 action is not"
            ),
        )),
        (None, _) => Err(route_error(
            route,
            format_args!("refreshes screen {key}, but no screen route renders it"),
        )),
        (Some(_), Some(_)) => Err(route_error(
            route,
            format_args!(
                "refreshes screen {key}, which several screen routes render; give each a screen of its own"
            ),
        )),
    }
}

#[cfg(feature = "engine")]
/// The screen's path is filled from the action's own parameters.
fn require_fillable(route: &Route, page: &ScreenRoute) -> Result<()> {
    let available = pattern::parameter_names(&route.path);
    let missing = pattern::parameter_names(&page.path)
        .into_iter()
        .find(|name| !available.contains(name));
    let Some(missing) = missing else {
        return Ok(());
    };
    Err(route_error(
        route,
        format_args!(
            "refreshes the screen at {}, whose `{{{missing}}}` it does not have",
            page.path
        ),
    ))
}
