use std::collections::BTreeMap;

use sofie_model::{App, FeatureDeclaration, Module, manifest_file};

use super::{Finding, Rule};
use crate::Date;

pub(super) fn check_flags(app: &App) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut seen: BTreeMap<&str, &FeatureDeclaration> = BTreeMap::new();
    for module in &app.modules {
        for declaration in &module.code.features.declarations {
            findings.extend(misnamed(module, declaration));
            findings.extend(outside_manifest(module, declaration));
            findings.extend(undated(declaration));
            let Some(name) = declaration.name.as_deref() else {
                continue;
            };
            if let Some(first) = seen.insert(name, declaration) {
                findings.push(finding(
                    Rule::FeatureFlag,
                    declaration,
                    format!("flag `{name}` is declared twice, here and at {}:{}, so one switch would gate both", first.file.display(), first.line),
                    "give each flag a name of its own",
                ));
            }
        }
        findings.extend(unknown_gates(module));
        findings.extend(unknown_menu_flags(module));
    }
    findings
}

fn misnamed(module: &Module, declaration: &FeatureDeclaration) -> Option<Finding> {
    let prefix = format!("{}.", module.krate.crate_name);
    let named = declaration.name.as_deref().filter(|name| {
        name.strip_prefix(&prefix)
            .is_some_and(|flag| !flag.is_empty() && !flag.contains('.'))
    });
    if named.is_some() {
        return None;
    }
    Some(finding(
        Rule::FeatureFlag,
        declaration,
        format!(
            "flag `{}` is not named `{prefix}<flag>` by a literal, so its stored state could not be told apart from another module's",
            declaration.constant.join("::")
        ),
        &format!("write `Feature::new(\"{prefix}<flag>\")`"),
    ))
}

fn outside_manifest(module: &Module, declaration: &FeatureDeclaration) -> Option<Finding> {
    let manifest = manifest_file(&module.krate.path);
    (declaration.file != manifest).then(|| {
        finding(
            Rule::FeatureFlag,
            declaration,
            format!(
                "flag `{}` is declared outside the module's manifest {}",
                declaration.constant.join("::"),
                manifest.display()
            ),
            "move the constant into src/module.rs, beside `MODULE`",
        )
    })
}

fn unknown_gates(module: &Module) -> Vec<Finding> {
    let features = &module.code.features;
    features
        .gates
        .iter()
        .filter(|gate| features.declaration_of(gate).is_none())
        .map(|gate| Finding {
            rule: Rule::FeatureFlag,
            file: gate.file.clone(),
            line: gate.line,
            message: format!(
                "`#[feature({})]` on `{}` names no flag this module declares exactly once, so `sofie gen` cannot gate it",
                gate.feature.join("::"),
                gate.function.join("::")
            ),
            fix: "declare the flag as a `Feature` constant in the module and name it here".to_owned(),
        })
        .collect()
}

fn unknown_menu_flags(module: &Module) -> Vec<Finding> {
    let menus = module.areas.iter().filter_map(|area| area.menu.as_ref());
    menus
        .flat_map(|menu| menu.entries.iter().map(move |entry| (menu, entry)))
        .filter(|(_, entry)| entry.feature.is_some() && module.menu_feature(entry).is_none())
        .map(|(menu, entry)| Finding {
            rule: Rule::FeatureFlag,
            file: menu.file.clone(),
            line: entry.line,
            message: format!(
                "menu entry `{}` is shown by `{}`, which names no flag this module declares exactly once",
                entry.label,
                entry.feature.as_deref().unwrap_or_default()
            ),
            fix: "declare the flag as a `Feature` constant in the module's manifest and name it here".to_owned(),
        })
        .collect()
}

#[must_use]
pub fn check_flag_ages(app: &App, today: Date) -> Vec<Finding> {
    let declarations = app
        .modules
        .iter()
        .flat_map(|module| &module.code.features.declarations);
    declarations
        .filter_map(|declaration| {
            let added = Date::parse(declaration.added.as_deref()?)?;
            let age = today.days_since(added);
            (age > i64::from(declaration.retire_after_days)).then(|| finding(
                Rule::FlagAge,
                declaration,
                format!(
                    "flag `{}` was added {age} days ago, past the {} days it declares",
                    declaration.constant.join("::"),
                    declaration.retire_after_days
                ),
                "switch it fully on or off, then remove the flag and keep only the code that state runs; the build shows every use left",
            ))
        })
        .collect()
}

fn undated(declaration: &FeatureDeclaration) -> Option<Finding> {
    let constant = declaration.constant.join("::");
    let message = match declaration.added.as_deref() {
        Some(added) if Date::parse(added).is_some() => return None,
        Some(added) => {
            format!("flag `{constant}` was added on `{added}`, which is no `YYYY-MM-DD` day")
        }
        None => format!(
            "flag `{constant}` declares no literal day it was added, so `sofie check` cannot tell when it outlives its {} days",
            declaration.retire_after_days
        ),
    };
    Some(finding(
        Rule::FlagAge,
        declaration,
        message,
        "write `.added(\"YYYY-MM-DD\")` with the day the flag was added",
    ))
}

fn finding(rule: Rule, declaration: &FeatureDeclaration, message: String, fix: &str) -> Finding {
    Finding {
        rule,
        file: declaration.file.clone(),
        line: declaration.line,
        message,
        fix: fix.to_owned(),
    }
}
