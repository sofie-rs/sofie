//! Only fixes that have exactly one form; nothing that needs a decision.

use std::path::{Path, PathBuf};

use sofie_check::{AllowList, Finding, Rule, missing_gen_exports};
use sofie_model::naming::relative_path;
use sofie_model::{App, Module};

use crate::check_command::check_app;
use crate::gen_command::{GenError, io_error, regenerate};
use crate::new_command::edits::with_feature_on;
use crate::new_command::task::tasks_wiring;
use crate::rustfmt::format_rust;
use crate::source_edits::{add_dependency, append_lines, declare_module, read, write};

pub struct FixReport {
    pub applied: Vec<String>,
    pub remaining: Vec<Finding>,
}

pub fn fix_app(root: &Path) -> Result<FixReport, GenError> {
    let app = sofie_model::load(root).map_err(GenError::Model)?;
    let mut applied = Vec::new();
    for module in &app.modules {
        applied.extend(declare_missing_modules(&app, module)?);
    }
    applied.extend(
        wire_generated_code(&app)?
            .into_iter()
            .map(|(_, change)| change),
    );
    applied.extend(add_app_dependencies(&app)?);
    let mut remaining = check_app(root)?;
    let only_stale = remaining.iter().all(|finding| is_stale(finding.rule));
    if !remaining.is_empty() && only_stale {
        applied.extend(regenerate(root)?);
        remaining = check_app(root)?;
    }
    // The dead code check runs only once the generated files are current.
    let removed = remove_stale_allowances(&app, &remaining)?;
    if !removed.is_empty() {
        applied.extend(removed);
        remaining = check_app(root)?;
    }
    Ok(FixReport { applied, remaining })
}

/// An allowance whose item is used or gone has nothing left to allow.
fn remove_stale_allowances(app: &App, findings: &[Finding]) -> Result<Vec<String>, GenError> {
    let mut files: Vec<(&Path, Vec<usize>)> = Vec::new();
    for finding in findings
        .iter()
        .filter(|finding| finding.rule == Rule::StaleAllowance)
    {
        match files.iter_mut().find(|(file, _)| *file == finding.file) {
            Some((_, lines)) => lines.push(finding.line),
            None => files.push((&finding.file, vec![finding.line])),
        }
    }
    let mut removed = Vec::new();
    for (file, lines) in files {
        let left = AllowList::without_entries(&read(file)?, &lines);
        if left.trim().is_empty() {
            std::fs::remove_file(file).map_err(|source| io_error(file, source))?;
        } else {
            write(file, left)?;
        }
        removed.push(format!(
            "removed {} stale allowance(s) from {}",
            lines.len(),
            shown(app, file)
        ));
    }
    Ok(removed)
}

fn is_stale(rule: Rule) -> bool {
    matches!(rule, Rule::StaleWiring | Rule::StaleDocs)
}

fn declare_missing_modules(app: &App, module: &Module) -> Result<Vec<String>, GenError> {
    let mut paths: Vec<Vec<&str>> = Vec::new();
    if module
        .manifest
        .as_ref()
        .is_some_and(|manifest| !manifest.declared)
    {
        paths.push(vec!["module"]);
    }
    if module
        .config
        .as_ref()
        .is_some_and(|config| !config.declared)
    {
        paths.push(vec!["config"]);
    }
    for area in &module.areas {
        if !area.declared {
            paths.push(vec!["areas", &area.area, "routes"]);
        }
        if area.menu.as_ref().is_some_and(|menu| !menu.declared) {
            paths.push(vec!["areas", &area.area, "menu"]);
        }
    }
    let root = module.krate.path.join("src").join("lib.rs");
    let mut applied = Vec::new();
    for path in paths {
        if let Some(file) = declare_module(&root, &path)? {
            applied.push(format!(
                "declared `{}` in {}",
                path.join("::"),
                shown(app, &file)
            ));
        }
    }
    Ok(applied)
}

/// What makes the generated code reachable, in its one form: a contract declaring and
/// exporting its `gen` module, and `main` handing the wiring's tasks to the runner. `sofie gen`
/// writes it too, so an app a generator or a hand edit leaves builds.
pub(crate) fn wire_generated_code(app: &App) -> Result<Vec<(PathBuf, String)>, GenError> {
    let mut applied = Vec::new();
    for module in &app.modules {
        applied.extend(export_generated(app, module)?);
    }
    for module in app.modules.iter().filter(|module| !module.tasks.is_empty()) {
        let manifest = module.krate.path.join("Cargo.toml");
        let text = read(&manifest)?;
        // A dependency written over several lines is left to the build's own error.
        if let Some(edited) = with_feature_on(&text, "sofie", "tasks")
            && edited != text
        {
            write(&manifest, &edited)?;
            let change = format!("turned sofie's `tasks` on in {}", shown(app, &manifest));
            applied.push((manifest, change));
        }
    }
    let declares_tasks = app.modules.iter().any(|module| !module.tasks.is_empty());
    // A `main` it cannot change is left to `sofie check`'s `task-runner` finding.
    if declares_tasks && let Ok(files) = tasks_wiring(app)? {
        for (file, text) in files {
            write(&file, format_rust(&text, &file).map_err(GenError::Rustfmt)?)?;
            let change = format!(
                "handed the wiring's tasks to the runner in {}",
                shown(app, &file)
            );
            applied.push((file, change));
        }
    }
    Ok(applied)
}

fn export_generated(app: &App, module: &Module) -> Result<Vec<(PathBuf, String)>, GenError> {
    let Some(contract) = &module.contract else {
        return Ok(Vec::new());
    };
    let lines = generated_export_lines(app, module);
    if lines.is_empty() {
        return Ok(Vec::new());
    }
    let root = contract.path.join("src").join("lib.rs");
    append_lines(&root, &lines)?;
    let change = format!("added `{}` to {}", lines.join(" "), shown(app, &root));
    Ok(vec![(root, change)])
}

/// What the contract's `lib.rs` lacks to declare and export the code `sofie gen` writes into it.
pub(crate) fn generated_export_lines(app: &App, module: &Module) -> Vec<String> {
    let missing = missing_gen_exports(app, module);
    let needs_gen = !missing.is_empty()
        || module
            .defined_areas
            .iter()
            .any(sofie_model::AreaDefinition::needs_generated_member_area);
    let mut lines = Vec::new();
    if needs_gen && !module.contract_declares_gen {
        lines.push("mod r#gen;".to_owned());
    }
    lines.extend(missing.iter().map(|name| format!("pub use r#gen::{name};")));
    lines
}

fn add_app_dependencies(app: &App) -> Result<Vec<String>, GenError> {
    let mut needed: Vec<(&str, &Path)> = Vec::new();
    for module in &app.modules {
        needed.push((module.krate.package.as_str(), module.krate.path.as_path()));
        // The wiring names a contract's areas only to sign their members in.
        if app.identity.is_some()
            && !module.defined_areas.is_empty()
            && let Some(contract) = &module.contract
        {
            needed.push((contract.package.as_str(), contract.path.as_path()));
        }
    }
    needed.sort_unstable();
    needed.dedup();
    let manifest = app.app.path.join("Cargo.toml");
    let mut applied = Vec::new();
    for (package, path) in needed {
        if app.depends_on(package) {
            continue;
        }
        let Some(relative) = relative_path(&app.app.path, path) else {
            continue;
        };
        add_dependency(&manifest, package, &relative)?;
        applied.push(format!(
            "added `{package}` to the [dependencies] of {}",
            shown(app, &manifest)
        ));
    }
    Ok(applied)
}

fn shown(app: &App, file: &Path) -> String {
    file.strip_prefix(&app.root)
        .unwrap_or(file)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use sofie_check::{Finding, Rule};
    use sofie_model::testing::{TemporaryDirectory, app};

    use super::remove_stale_allowances;

    fn stale(file: std::path::PathBuf, line: usize) -> Finding {
        Finding {
            rule: Rule::StaleAllowance,
            file,
            line,
            message: String::new(),
            fix: String::new(),
        }
    }

    #[test]
    fn removes_each_stale_allowance_from_its_own_file() {
        let directory = TemporaryDirectory::new("sofie-fix-stale");
        let entries = "[[item]]\nname = \"a\"\n\n[[item]]\nname = \"b\"\n";
        let items = directory.write("dead-code-allow.toml", entries);
        let locks = directory.write("lock-allow.toml", entries);
        let findings = [stale(items.clone(), 1), stale(locks.clone(), 4)];

        let removed =
            remove_stale_allowances(&app(directory.root(), Vec::new()), &findings).unwrap();

        assert_eq!(removed.len(), 2);
        assert_eq!(
            std::fs::read_to_string(items).unwrap(),
            "[[item]]\nname = \"b\"\n"
        );
        assert_eq!(
            std::fs::read_to_string(locks).unwrap(),
            "[[item]]\nname = \"a\"\n"
        );
    }
}
