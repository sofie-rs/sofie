use std::path::{Path, PathBuf};

use sofie_model::naming::{WEB_AREA, file_stem, relative_path, screen_key};
use sofie_model::{App, AreaContribution, KnownArea, Module, ScreenOverride};

use super::{Finding, Rule};

pub(super) fn check_wiring(app: &App) -> Vec<Finding> {
    let areas = app.areas();
    let mut findings: Vec<Finding> = duplicate_areas(&areas).collect();
    findings.extend(
        areas
            .iter()
            .filter_map(|area| member_area_contract_missing(app, area)),
    );
    for module in &app.modules {
        if !app.depends_on(&module.krate.package) {
            findings.push(missing_module_dependency(app, module));
        }
        for area in module.areas.iter().filter(|area| area.declared) {
            findings.extend(unknown_area(&areas, area));
            findings.extend(
                area.overrides
                    .iter()
                    .filter_map(|replacement| unknown_override(app, area, replacement)),
            );
        }
        findings.extend(embedded_files(module).filter_map(|file| unembeddable(app, file)));
    }
    findings
}

fn duplicate_areas<'app>(areas: &'app [KnownArea<'app>]) -> impl Iterator<Item = Finding> + 'app {
    areas.iter().enumerate().filter_map(|(index, area)| {
        let definition = area.definition?;
        areas[..index]
            .iter()
            .any(|earlier| earlier.name == area.name)
            .then(|| Finding {
                rule: Rule::AreaDefinition,
                file: definition.file.clone(),
                line: definition.line,
                message: format!(
                    "area `{}` is already defined, so its routes could not tell which guard applies",
                    area.name
                ),
                fix: format!(
                    "define each area once, in the contract of the module that owns its principal, \
                     under a name other than `{WEB_AREA}`"
                ),
            })
    })
}

fn missing_module_dependency(app: &App, module: &Module) -> Finding {
    let path = relative_path(&app.app.path, &module.krate.path)
        .unwrap_or_else(|| module.krate.path.display().to_string());
    let package = &module.krate.package;
    Finding {
        rule: Rule::AppDependency,
        file: app.app.path.join("Cargo.toml"),
        line: 1,
        message: format!(
            "the app does not depend on module `{package}`, so its wiring would not compile"
        ),
        fix: format!("add `{package} = {{ path = \"{path}\" }}` to [dependencies]"),
    }
}

fn unknown_area(areas: &[KnownArea], area: &AreaContribution) -> Option<Finding> {
    if areas.iter().any(|known| known.name == area.area) {
        return None;
    }
    let names: Vec<&str> = areas.iter().map(|known| known.name).collect();
    Some(Finding {
        rule: Rule::UnknownArea,
        file: area.routes_file.clone(),
        line: 1,
        message: format!("no area is named `{}`", area.area),
        fix: format!(
            "define it in a contract crate, `impl Area for <Type>` with `const NAME: \
             &'static str = \"{}\";`, or move the routes to an area that exists: {}",
            area.area,
            names.join(", ")
        ),
    })
}

/// The wiring names every member area of every contract, whether or not a module routes in it.
fn member_area_contract_missing(app: &App, area: &KnownArea) -> Option<Finding> {
    let contract = area.contract?;
    let definition = area.definition?;
    if app.identity.is_none() || !definition.is_member_area() || app.depends_on(contract) {
        return None;
    }
    Some(Finding {
        rule: Rule::AppDependency,
        file: definition.file.clone(),
        line: definition.line,
        message: format!(
            "the app does not depend on `{contract}`, which defines area `{}`, so its wiring \
             cannot name the area sofie-identity signs members in at",
            area.name
        ),
        fix: format!("add `{contract}` to the app's [dependencies]"),
    })
}

fn unknown_override(
    app: &App,
    area: &AreaContribution,
    replacement: &ScreenOverride,
) -> Option<Finding> {
    let key = screen_key(
        &replacement.module,
        &area.area,
        &file_stem(&replacement.file),
    );
    let built_in = app
        .identity
        .as_ref()
        .map_or(&[][..], |identity| identity.screens.as_slice());
    if built_in.contains(&key) {
        return None;
    }
    Some(Finding {
        rule: Rule::ScreenOverride,
        file: replacement.file.clone(),
        line: 1,
        message: format!(
            "this template would replace built-in screen `{key}`, which does not exist"
        ),
        fix: format!(
            "keep a module's own screens directly in `screens/`; a folder in it may only replace \
             a built-in screen of a framework crate the app depends on: {}",
            if built_in.is_empty() {
                "there are none".to_owned()
            } else {
                built_in.join(", ")
            }
        ),
    })
}

fn embedded_files(module: &Module) -> impl Iterator<Item = &Path> {
    let screens = module.areas.iter().flat_map(|area| {
        area.screens.iter().map(PathBuf::as_path).chain(
            area.overrides
                .iter()
                .map(|replacement| replacement.file.as_path()),
        )
    });
    module
        .migrations
        .iter()
        .map(PathBuf::as_path)
        .chain(screens)
}

fn unembeddable(app: &App, file: &Path) -> Option<Finding> {
    if app.embedded_path(file).is_some() {
        return None;
    }
    Some(Finding {
        rule: Rule::EmbeddedPath,
        file: file.to_path_buf(),
        line: 1,
        message: format!(
            "this file cannot be spelled relative to {} in UTF-8, so the wiring cannot embed it",
            app.app.path.display()
        ),
        fix: "keep modules under the app root, with UTF-8 file and directory names".to_owned(),
    })
}
