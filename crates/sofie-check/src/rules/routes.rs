use std::path::Path;

use sofie_model::naming::{RouteSite, handler_module};
use sofie_model::{App, Module, Then};

use super::{Finding, Rule};

pub(super) fn check_routes(app: &App) -> Vec<Finding> {
    let sites = app.route_sites();
    let mut findings = undeclared_route_files(&app.modules);
    for &site in &sites {
        findings.extend(misrouted_handler(site));
        findings.extend(missing_screen(site));
        findings.extend(unrouted_then_screen(site));
    }
    findings
}

fn undeclared_route_files(modules: &[Module]) -> Vec<Finding> {
    let areas = modules.iter().flat_map(|module| &module.areas);
    let mut findings = Vec::new();
    for area in areas {
        if !area.declared {
            findings.push(undeclared(
                &area.routes_file,
                &area.area,
                "routes",
                "its routes",
            ));
        }
        if let Some(menu) = area.menu.as_ref().filter(|menu| !menu.declared) {
            findings.push(undeclared(
                &menu.file,
                &area.area,
                "menu",
                "its menu entries",
            ));
        }
    }
    findings
}

fn undeclared(file: &Path, area: &str, module: &str, what: &str) -> Finding {
    Finding {
        rule: Rule::UndeclaredRoutes,
        file: file.to_path_buf(),
        line: 1,
        message: format!(
            "the `{area}` {module} file is not reachable through `mod` declarations, so the \
             build would silently leave {what} out"
        ),
        fix: format!(
            "declare it in the crate root: `mod areas {{ pub mod {area} {{ pub mod {module}; }} }}`, \
             or with the same `mod` items in `areas/mod.rs` and the area's `mod.rs`"
        ),
    }
}

fn misrouted_handler(site: RouteSite<'_>) -> Option<Finding> {
    let expected_root = handler_module(site.route.method);
    let handler = site.route.handler.join("::");
    if site.route.is_resource()
        || site.route.handler.first().map(String::as_str) == Some(expected_root)
    {
        return None;
    }
    // A bare name the model could not resolve, such as one behind two globs, has no known
    // module, so it could be a writing action under GET.
    if site.route.handler.len() == 1 {
        return Some(Finding {
            rule: Rule::RouteKind,
            file: site.area.routes_file.clone(),
            line: site.route.line,
            message: format!(
                "{} {} is handled by `{handler}`, which cannot be traced to `queries` or \
                 `actions`, so whether a GET changes state cannot be checked",
                site.route.method.name(),
                site.route.path
            ),
            fix: format!(
                "name it by its module, as `{expected_root}::{handler}`, or import it by name \
                 instead of through a glob"
            ),
        });
    }
    Some(Finding {
        rule: Rule::RouteKind,
        file: site.area.routes_file.clone(),
        line: site.route.line,
        message: format!(
            "{} {} is handled by `{handler}`, which is not under `{expected_root}`: a GET must \
             never change state and an action must never be a GET",
            site.route.method.name(),
            site.route.path
        ),
        fix: format!(
            "route a query under GET and an action under POST, PUT, PATCH or DELETE, so this \
             handler belongs in `{expected_root}`"
        ),
    })
}

fn missing_screen(site: RouteSite<'_>) -> Option<Finding> {
    let name = site
        .route
        .screen
        .as_deref()
        .filter(|_| !site.route.is_resource())?;
    let template = site.area.screen_file(name);
    if site.area.screens.contains(&template) {
        return None;
    }
    Some(Finding {
        rule: Rule::MissingScreen,
        file: site.area.routes_file.clone(),
        line: site.route.line,
        message: format!(
            "{} renders screen `{name}`, but {} does not exist, so the app would refuse to serve",
            site.route.path,
            template.display()
        ),
        fix: format!("create {} or name an existing screen", template.display()),
    })
}

fn unrouted_then_screen(site: RouteSite<'_>) -> Option<Finding> {
    let Some(Then::Screen { screen: name, .. }) = &site.route.then else {
        return None;
    };
    let rendering = site
        .area
        .routes
        .iter()
        .filter(|route| route.screen.as_ref() == Some(name))
        .count();
    let problem = match rendering {
        1 => return None,
        0 => "no `.screen(…)` route in this area renders it".to_owned(),
        _ => format!("{rendering} `.screen(…)` routes render it, so its loader is ambiguous"),
    };
    Some(Finding {
        rule: Rule::ThenScreen,
        file: site.area.routes_file.clone(),
        line: site.route.line,
        message: format!(
            "{} {} refreshes screen `{name}`, but {problem}; the app would refuse to serve",
            site.route.method.name(),
            site.route.path
        ),
        fix: format!("route screen `{name}` exactly once in this file, or refresh another screen"),
    })
}
