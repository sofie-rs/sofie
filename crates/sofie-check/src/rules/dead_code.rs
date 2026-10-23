use std::collections::BTreeMap;

use sofie_model::usage::{Mentions, PublicItem};
use sofie_model::{Crate, Dependency, ModelError, Source};

use super::allowances::{AllowList, Allowed};
use super::manifest::declaration_line;
use super::{Finding, Rule};
use crate::workspace::read_text;

/// Lower it as dead code is deleted; never raise it.
pub const MAX_UNUSED_PUBLIC_RATIO: f64 = 0.02;

struct DeadCode<'model> {
    krate: &'model Crate,
    public_items: usize,
    unused_items: Vec<(&'model Source, &'model PublicItem)>,
    unused_dependencies: Vec<&'model Dependency>,
}

pub fn check_dead_code(
    checked: &[&Crate],
    context: &[&Crate],
    allowed: &AllowList,
    ceiling: f64,
) -> Result<Vec<Finding>, ModelError> {
    let mut mentions = Mentions::default();
    for source in checked
        .iter()
        .chain(context)
        .flat_map(|krate| all_files(krate))
    {
        mentions.add(&source.mentions);
    }
    let definitions = definitions(checked);
    let dead_code: Vec<DeadCode> = checked
        .iter()
        .map(|krate| dead_code_of(krate, &mentions, &definitions))
        .collect();
    let mut findings = Vec::new();
    for crate_dead_code in &dead_code {
        findings.extend(item_findings(crate_dead_code, allowed));
        findings.extend(dependency_findings(crate_dead_code, allowed)?);
        findings.extend(ratio_finding(crate_dead_code, ceiling));
    }
    findings.extend(stale_allowances(&dead_code, allowed));
    Ok(findings)
}

fn all_files(krate: &Crate) -> impl Iterator<Item = &Source> {
    krate.sources.iter().chain(&krate.tests_and_examples)
}

/// How many tracked items define each name: their own definition is a
/// mention that is not a use.
fn definitions<'model>(checked: &[&'model Crate]) -> BTreeMap<&'model str, usize> {
    let mut definitions = BTreeMap::new();
    for source in checked.iter().flat_map(|krate| &krate.sources) {
        for item in &source.public_items {
            *definitions.entry(item.name.as_str()).or_default() += 1;
        }
    }
    definitions
}

fn dead_code_of<'model>(
    krate: &'model Crate,
    mentions: &Mentions,
    definitions: &BTreeMap<&str, usize>,
) -> DeadCode<'model> {
    // What `sofie gen` writes is exported for others to call, used or not.
    let generated = krate.gen_file();
    let items: Vec<(&Source, &PublicItem)> = krate
        .sources
        .iter()
        .filter(|source| source.file != generated)
        .flat_map(|source| source.public_items.iter().map(move |item| (source, item)))
        .collect();
    DeadCode {
        krate,
        public_items: items.len(),
        unused_items: items
            .into_iter()
            .filter(|(_, item)| {
                mentions.count(&item.name)
                    <= definitions
                        .get(item.name.as_str())
                        .copied()
                        .unwrap_or_default()
            })
            .collect(),
        unused_dependencies: krate
            .dependencies
            .iter()
            .filter(|dependency| !names_crate(krate, &dependency.crate_name))
            .collect(),
    }
}

/// A re-export counts: the dependency is what makes it compile.
fn names_crate(krate: &Crate, name: &str) -> bool {
    krate.sources.iter().any(|source| {
        source.mentions.count(name) > 0
            || source
                .paths
                .iter()
                .any(|path| path.segments.first().is_some_and(|first| first == name))
    })
}

fn item_findings<'model>(
    dead_code: &'model DeadCode<'model>,
    allowed: &'model AllowList,
) -> impl Iterator<Item = Finding> + 'model {
    let package = &dead_code.krate.package;
    dead_code
        .unused_items
        .iter()
        .filter(|(_, item)| !allowed.allows(Allowed::Item, package, &item.name))
        .map(move |(source, item)| Finding {
            rule: Rule::UnusedItem,
            file: source.file.clone(),
            line: item.line,
            message: format!(
                "`{}` is `pub` but nothing names it outside its definition, so it is dead code",
                item.name
            ),
            fix: format!(
                "delete it, or list it in dead-code-allow.toml as `[[item]]` of package \
                 `{package}` with the reason it stays"
            ),
        })
}

fn dependency_findings(
    dead_code: &DeadCode<'_>,
    allowed: &AllowList,
) -> Result<Vec<Finding>, ModelError> {
    let krate = dead_code.krate;
    let unused: Vec<&Dependency> = dead_code
        .unused_dependencies
        .iter()
        .copied()
        .filter(|dependency| {
            !allowed.allows(Allowed::Dependency, &krate.package, &dependency.package)
        })
        .collect();
    if unused.is_empty() {
        return Ok(Vec::new());
    }
    let manifest = krate.path.join("Cargo.toml");
    let text = read_text(&manifest)?;
    Ok(unused
        .into_iter()
        .map(|dependency| Finding {
            rule: Rule::UnusedDependency,
            file: manifest.clone(),
            line: declaration_line(&text, &dependency.package),
            message: format!(
                "`{}` is in [dependencies] but no source of `{}` names `{}`",
                dependency.package, krate.package, dependency.crate_name
            ),
            fix: "remove it, move it to [dev-dependencies] if only tests use it, or list it in \
                  dead-code-allow.toml as `[[dependency]]` with the reason it stays"
                .to_owned(),
        })
        .collect())
}

/// Below the size where the ceiling allows one unused item the share says
/// nothing, and each unused item is already reported unless allowed with a reason.
fn ratio_finding(dead_code: &DeadCode<'_>, ceiling: f64) -> Option<Finding> {
    if crate::count_as_f64(dead_code.public_items) * ceiling < 1.0 {
        return None;
    }
    let unused = dead_code.unused_items.len();
    let ratio = crate::count_as_f64(unused) / crate::count_as_f64(dead_code.public_items.max(1));
    if ratio <= ceiling {
        return None;
    }
    Some(Finding {
        rule: Rule::UnusedRatio,
        file: dead_code.krate.path.join("Cargo.toml"),
        line: 1,
        message: format!(
            "{unused} of {} `pub` items of `{}` are unused, allowed ones included: {:.1}% is above \
             the ceiling of {:.1}%",
            dead_code.public_items,
            dead_code.krate.package,
            ratio * 100.0,
            ceiling * 100.0
        ),
        fix: "delete unused items until the share is under the ceiling".to_owned(),
    })
}

fn stale_allowances(dead_code: &[DeadCode<'_>], allowed: &AllowList) -> Vec<Finding> {
    allowed
        .allowances
        .iter()
        .filter(|allowance| {
            !dead_code.iter().any(|crate_dead_code| {
                crate_dead_code.krate.package == allowance.package
                    && match allowance.kind {
                        Allowed::Item => crate_dead_code
                            .unused_items
                            .iter()
                            .any(|(_, item)| item.name == allowance.name),
                        Allowed::Dependency => crate_dead_code
                            .unused_dependencies
                            .iter()
                            .any(|dependency| dependency.package == allowance.name),
                    }
            })
        })
        .map(|allowance| Finding {
            rule: Rule::StaleAllowance,
            file: allowed.file.clone(),
            line: allowance.line,
            message: format!(
                "`{}` of `{}` is allowed as unused, but it is used or no longer exists",
                allowance.name, allowance.package
            ),
            fix: "delete this entry".to_owned(),
        })
        .collect()
}
