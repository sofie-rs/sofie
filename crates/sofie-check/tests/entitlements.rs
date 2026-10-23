use std::path::PathBuf;

use sofie_check::{Rule, Screens};
use sofie_model::testing::{TemporaryDirectory, app, area, module, route};
use sofie_model::{App, EntitlementDeclaration, Method, Modifier, Route};

fn entitled(path: &str, written: &str, line: usize) -> Route {
    let mut entitled = route(Method::Get, path, "queries::exports", line);
    entitled
        .modifiers
        .push(Modifier::Entitled(written.to_owned()));
    entitled
}

fn entitlement_findings(app: &App) -> Vec<(String, usize)> {
    sofie_check::check(app, &Screens::read(&app.modules))
        .into_iter()
        .filter(|finding| finding.rule == Rule::Entitlement)
        .map(|finding| {
            let file = finding.file.file_name().unwrap_or_default();
            (file.to_string_lossy().into_owned(), finding.line)
        })
        .collect()
}

#[test]
fn routes_and_screens_name_only_entitlements_the_app_declares() {
    let directory = TemporaryDirectory::new("sofie-check-entitlements");
    let mut project = module("project", directory.path("modules/project"));
    let mut web = area(
        &project,
        "web",
        vec![
            entitled("/exports", "crate::plans::EXPORT", 3),
            entitled("/projects", "plans::PROJECTS.entitlement()", 4),
            entitled("/imports", "plans::IMPORT", 5),
        ],
    );
    web.screens = vec![directory.write(
        "modules/project/src/areas/web/screens/exports.html",
        "<p>Exports</p>\n{#if entitled(\"project.export\")}<p>Export</p>{/if}\n\
         {#if entitled(\"project.import\")}<p>Import</p>{/if}\n",
    )];
    project.areas.push(web);
    let declared = |constant: &str, name: &str, line| EntitlementDeclaration {
        constant: vec!["plans".to_owned(), constant.to_owned()],
        name: name.to_owned(),
        file: PathBuf::from("modules/project/src/plans.rs"),
        line,
    };
    project.code.entitlements = vec![
        declared("EXPORT", "project.export", 3),
        declared("PROJECTS", "project.projects", 4),
    ];

    let findings = entitlement_findings(&app(directory.root(), vec![project]));

    let found = |file: &str, line| (file.to_owned(), line);
    assert_eq!(findings, [found("routes.rs", 5), found("exports.html", 3)]);
}
