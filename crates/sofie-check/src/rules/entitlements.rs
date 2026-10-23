use std::path::Path;

use sofie_engine::asked_entitlements;
use sofie_model::{App, Modifier};

use super::{Finding, Rule};
use crate::screens::Screens;

pub(super) fn check_entitlements(app: &App, screens: &Screens) -> Vec<Finding> {
    let mut findings = unknown_route_entitlements(app);
    let screen_files = app
        .modules
        .iter()
        .flat_map(|module| &module.areas)
        .flat_map(|area| {
            let overrides = area.overrides.iter().map(|replacement| &replacement.file);
            area.screens.iter().chain(overrides)
        });
    for file in screen_files {
        findings.extend(unknown_template_entitlements(app, screens, file));
    }
    findings
}

fn unknown_route_entitlements(app: &App) -> Vec<Finding> {
    let mut findings = Vec::new();
    for site in app.route_sites() {
        for modifier in &site.route.modifiers {
            let Modifier::Entitled(written) = modifier else {
                continue;
            };
            if app.entitlements().written(written).is_some() {
                continue;
            }
            findings.push(Finding {
                rule: Rule::Entitlement,
                file: site.area.routes_file.clone(),
                line: site.route.line,
                message: format!(
                    "{} is entitled by `{written}`, which names no `Entitlement`, `Limit` or \
                     `Quota` the app declares with a literal name, so no plan is known to hold it",
                    site.route.path
                ),
                fix: "declare it in a module, such as `pub const EXPORT: Entitlement = \
                      Entitlement::new(\"project.export\");`, and name that constant"
                    .to_owned(),
            });
        }
    }
    findings
}

fn unknown_template_entitlements(app: &App, screens: &Screens, file: &Path) -> Vec<Finding> {
    let Ok(screen) = screens.screen(file) else {
        return Vec::new();
    };
    let name = file.display().to_string();
    let Ok(asked) = asked_entitlements(&name, &screen.source, screens.components()) else {
        return Vec::new();
    };
    asked
        .into_iter()
        .filter(|asked| {
            !app.entitlements()
                .iter()
                .any(|declared| declared.name == asked.name)
        })
        .map(|asked| Finding {
            rule: Rule::Entitlement,
            file: file.to_path_buf(),
            line: asked.line,
            message: format!(
                "`entitled(\"{}\")` names no entitlement the app declares, so what it \
                 guards is shown to no one",
                asked.name
            ),
            fix: "name an `Entitlement`, `Limit` or `Quota` a module declares, or declare this one"
                .to_owned(),
        })
        .collect()
}
