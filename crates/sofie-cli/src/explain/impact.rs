//! What a change to an item touches, so an agent knows what else to read, run and update.

use sofie_model::naming::{RouteSite, contract_package, screen_key};
use sofie_model::{Aggregate, App, AreaContribution, Module, ReadModel, Source, Then};

use super::{mentions, shown_screens};

const HEADER: &str = "  change touches:";

pub(super) fn aggregate(
    app: &App,
    module: &Module,
    aggregate: &Aggregate,
    sites: &[RouteSite],
) -> Vec<String> {
    let models: Vec<&str> = module
        .code
        .read_models
        .iter()
        .filter(|model| model.from.as_deref() == Some(aggregate.name.as_str()))
        .map(|model| model.name.as_str())
        .collect();
    let mut names = vec![aggregate.name.as_str()];
    names.extend(&models);
    let mut touched: Vec<String> = models
        .iter()
        .map(|model| format!("    read model {model}"))
        .collect();
    touched.extend(routes_and_screens(module, sites, &names));
    if !aggregate.exposed.is_empty() {
        touched.extend(contract_readers(app, module, &aggregate.name));
    }
    touched.push(format!(
        "    migration: a new file under {} for a changed column",
        module.krate.path.join("migrations").display()
    ));
    touched.extend(tests(app, &aggregate.name));
    with_header(touched)
}

pub(super) fn read_model(
    app: &App,
    module: &Module,
    model: &ReadModel,
    sites: &[RouteSite],
) -> Vec<String> {
    let mut touched = routes_and_screens(module, sites, &[model.name.as_str()]);
    touched.extend(tests(app, &model.name));
    with_header(touched)
}

/// An action refreshing the screen sends it a fragment of its markup by id.
pub(super) fn screen(area: &AreaContribution, stem: &str, sites: &[RouteSite]) -> Vec<String> {
    let touched = sites
        .iter()
        .filter(|site| std::ptr::eq(site.area, area))
        .filter_map(|site| match &site.route.then {
            Some(Then::Screen { screen, fragment }) if screen == stem => Some(format!(
                "    action {} (refreshes `#{fragment}`)",
                site.name()
            )),
            _ => None,
        })
        .collect();
    with_header(touched)
}

pub(super) fn route(app: &App, site: &RouteSite) -> Vec<String> {
    let handler = site.route.handler.last().map_or("", String::as_str);
    let mut touched = Vec::new();
    if site.module.contract_declares_gen {
        touched.push(format!(
            "    contract route builder {}::routes::{}::{handler}",
            contract_package(&site.module.krate.package).replace('-', "_"),
            site.area.area
        ));
    }
    touched.extend(tests(app, handler));
    with_header(touched)
}

/// Routes whose handler takes or answers one of `names`, and the screens they show.
fn routes_and_screens(module: &Module, sites: &[RouteSite], names: &[&str]) -> Vec<String> {
    let mut routes = Vec::new();
    let mut screens = Vec::new();
    for site in sites
        .iter()
        .filter(|site| std::ptr::eq(site.module, module))
    {
        let Some(function) = module.code.function(&site.route.handler) else {
            continue;
        };
        let types = function.arguments.iter().chain(&function.output);
        if !types
            .into_iter()
            .any(|ty| names.iter().any(|name| mentions(ty, name)))
        {
            continue;
        }
        routes.push(format!("    route {}", site.name()));
        screens.extend(shown_screens(site).into_iter().map(|screen| {
            format!(
                "    screen {}",
                screen_key(&module.krate.crate_name, &site.area.area, &screen)
            )
        }));
    }
    screens.sort();
    screens.dedup();
    routes.extend(screens);
    routes
}

/// An exposed aggregate is looked up through the contract by every crate depending on it.
fn contract_readers(app: &App, module: &Module, name: &str) -> Vec<String> {
    let contract = contract_package(&module.krate.package);
    let mut touched = vec![format!(
        "    contract lookup {}::{name}",
        contract.replace('-', "_")
    )];
    let readers = app
        .modules
        .iter()
        .filter(|other| other.krate.package != module.krate.package)
        .map(|other| &other.krate)
        .chain(std::iter::once(&app.app))
        .filter(|krate| krate.depends_on(&contract));
    touched.extend(
        readers.map(|krate| format!("    crate {} (depends on {contract})", krate.package)),
    );
    touched
}

fn tests(app: &App, name: &str) -> Vec<String> {
    let crates = app
        .modules
        .iter()
        .flat_map(|module| std::iter::once(&module.krate).chain(&module.contract))
        .chain(std::iter::once(&app.app));
    crates
        .flat_map(|krate| &krate.tests_and_examples)
        .filter(|source: &&Source| source.mentions.count(name) > 0)
        .map(|source| format!("    test {}", source.file.display()))
        .collect()
}

fn with_header(touched: Vec<String>) -> Vec<String> {
    if touched.is_empty() {
        return touched;
    }
    std::iter::once(HEADER.to_owned()).chain(touched).collect()
}
