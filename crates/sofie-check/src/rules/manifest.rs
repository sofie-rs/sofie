use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sofie_model::{App, Crate, Module};

use super::{Finding, Rule, database, outgoing, tiers};

pub(super) struct DependencySite<'model> {
    pub module: &'model Module,
    pub krate: &'model Crate,
    pub is_contract: bool,
    pub dependency: &'model str,
    pub manifest: PathBuf,
    pub line: usize,
}

pub(super) fn check_manifests(app: &App) -> Vec<Finding> {
    let mut findings = Vec::new();
    let graph = LockedGraph::read(&app.root).unwrap_or_else(|(lock, error)| {
        findings.push(Finding {
            rule: Rule::Outgoing,
            file: lock,
            line: 1,
            message: format!("the lock file cannot be read: {error}"),
            fix: "make the file readable UTF-8 text, or delete it for Cargo to write again"
                .to_owned(),
        });
        LockedGraph::default()
    });
    let outgoing = OutgoingDependencies { app, graph: &graph };
    let sites = app.modules.iter().flat_map(|module| {
        let contract = module.contract.iter().map(|contract| (contract, true));
        std::iter::once((&module.krate, false))
            .chain(contract)
            .flat_map(move |(krate, is_contract)| dependency_sites(module, krate, is_contract))
    });
    for site in sites {
        findings.extend(module_dependency(&site, &app.modules));
        if site.dependency == app.app.package {
            findings.push(tiers::app_dependency_finding(&site));
        }
        if site.dependency == IDENTITY_PACKAGE && app.identity.is_none() {
            findings.push(identity_without_app(&site));
        }
        findings.extend(outgoing.finding(&site.manifest, site.line, site.dependency));
        if database::is_driver_crate(site.dependency) {
            findings.push(database::finding(
                &site.manifest,
                site.line,
                site.dependency,
            ));
        }
    }
    findings.extend(outgoing.of_app());
    findings
}

/// A vendor SDK opens its connections through the HTTP crate it depends on, so the
/// resolved graph is walked past every package but the framework's and the app's own.
struct OutgoingDependencies<'model> {
    app: &'model App,
    graph: &'model LockedGraph,
}

impl OutgoingDependencies<'_> {
    fn finding(&self, manifest: &Path, line: usize, dependency: &str) -> Option<Finding> {
        if outgoing::is_client_crate(dependency) {
            return Some(outgoing::finding(manifest, line, dependency));
        }
        if self.is_checked_on_its_own(dependency) {
            return None;
        }
        let client = self.graph.reaches(
            dependency,
            |package| !self.is_checked_on_its_own(package),
            outgoing::is_pulled_in_client,
        )?;
        Some(outgoing::pulled_in_finding(
            manifest, line, dependency, client,
        ))
    }

    fn is_checked_on_its_own(&self, package: &str) -> bool {
        package == "sofie"
            || package.starts_with("sofie-")
            || package == self.app.app.package
            || self.app.modules.iter().any(|module| {
                std::iter::once(&module.krate)
                    .chain(&module.contract)
                    .any(|krate| krate.package == package)
            })
    }

    fn of_app(&self) -> Vec<Finding> {
        let (manifest, text) = manifest_text(&self.app.app);
        self.app
            .app
            .dependencies
            .iter()
            .filter_map(|dependency| {
                let line = declaration_line(&text, &dependency.package);
                self.finding(&manifest, line, &dependency.package)
            })
            .collect()
    }
}

const IDENTITY_PACKAGE: &str = "sofie-identity";

/// The wiring signs members in only for an app that names sofie-identity itself.
fn identity_without_app(site: &DependencySite) -> Finding {
    Finding {
        rule: Rule::AppDependency,
        file: site.manifest.clone(),
        line: site.line,
        message: format!(
            "`{}` depends on `{IDENTITY_PACKAGE}`, but the app does not, so `sofie gen` wires no \
             sign-in, sessions or member areas and its areas admit nobody",
            site.krate.package
        ),
        fix: format!(
            "add `{IDENTITY_PACKAGE}.workspace = true` to the app's `[dependencies]`, then run \
             `sofie gen`"
        ),
    }
}

fn dependency_sites<'model>(
    module: &'model Module,
    krate: &'model Crate,
    is_contract: bool,
) -> Vec<DependencySite<'model>> {
    let (manifest, text) = manifest_text(krate);
    krate
        .dependencies
        .iter()
        .map(|dependency| DependencySite {
            module,
            krate,
            is_contract,
            dependency: &dependency.package,
            manifest: manifest.clone(),
            line: declaration_line(&text, &dependency.package),
        })
        .collect()
}

/// The model was loaded from this manifest, so failing to read it again adds
/// nothing; a finding still points at the file, at line 1.
pub(super) fn manifest_text(krate: &Crate) -> (PathBuf, String) {
    let manifest = krate.path.join("Cargo.toml");
    let text = fs::read_to_string(&manifest).unwrap_or_default();
    (manifest, text)
}

/// Only in a dependency table: a `[features]` key may carry a dependency's name.
pub(super) fn declaration_line(manifest_text: &str, dependency: &str) -> usize {
    let mut in_dependency_table = false;
    for (index, line) in manifest_text.lines().enumerate() {
        let line = line.trim_start();
        if let Some(table) = table_header(line) {
            if declares_table(table, dependency) {
                return index + 1;
            }
            in_dependency_table = is_dependency_table(table);
            continue;
        }
        let declares_key = line
            .strip_prefix(dependency)
            .is_some_and(|rest| rest.trim_start().starts_with(['=', '.']));
        if in_dependency_table && (declares_key || renames_package(line, dependency)) {
            return index + 1;
        }
    }
    1
}

fn table_header(line: &str) -> Option<&str> {
    let table = line.strip_prefix('[')?.split(']').next()?;
    Some(table.trim())
}

/// `dependencies`, `dev-dependencies`, `build-dependencies`, each also under `target.<cfg>`.
fn is_dependency_table(table: &str) -> bool {
    table
        .rsplit('.')
        .next()
        .is_some_and(|last| last.ends_with("dependencies"))
}

fn declares_table(table: &str, dependency: &str) -> bool {
    table
        .rsplit_once('.')
        .is_some_and(|(parent, name)| name == dependency && is_dependency_table(parent))
}

fn renames_package(line: &str, dependency: &str) -> bool {
    let quoted = format!("\"{dependency}\"");
    line.match_indices("package").any(|(start, key)| {
        line[start + key.len()..]
            .trim_start()
            .strip_prefix('=')
            .is_some_and(|value| value.trim_start().starts_with(&quoted))
    })
}

/// A contract depends on no module, its own included, so it can be served alone after a split.
fn module_dependency(site: &DependencySite<'_>, modules: &[Module]) -> Option<Finding> {
    let target = modules.iter().find(|module| {
        module.krate.package == site.dependency
            || module
                .contract
                .as_ref()
                .is_some_and(|contract| contract.package == site.dependency)
    })?;
    let own_module = std::ptr::eq(target, site.module);
    if site.is_contract {
        return Some(module_dependency_finding(site, target));
    }
    if own_module {
        return None;
    }
    if target.krate.package == site.dependency {
        return Some(module_dependency_finding(site, target));
    }
    tiers::check_direction(site, target)
}

fn module_dependency_finding(site: &DependencySite<'_>, target: &Module) -> Finding {
    let (message, fix) = if site.is_contract {
        (
            format!(
                "contract `{}` depends on `{}` of module `{}`; a contract depends on no module, \
                 so it can be moved or served remotely on its own",
                site.krate.package, site.dependency, target.krate.package
            ),
            "keep in the contract only what it publishes itself; let the module, not its \
             contract, depend on other contracts"
                .to_owned(),
        )
    } else {
        (
            format!(
                "module `{}` depends on module `{}` itself, which couples it to that module's \
                 internals instead of only to its contract",
                site.krate.package, site.dependency
            ),
            format!(
                "depend on `{}`'s contract crate instead of its implementation",
                site.dependency
            ),
        )
    };
    Finding {
        rule: Rule::ModuleDependency,
        file: site.manifest.clone(),
        line: site.line,
        message,
        fix,
    }
}

const LOCK_FILE: &str = "Cargo.lock";

/// Each package's dependencies as `Cargo.lock` resolved them, every version of a name merged.
#[derive(Default)]
struct LockedGraph {
    dependencies: BTreeMap<String, BTreeSet<String>>,
}

impl LockedGraph {
    /// The workspace's lock sits at or above the app root; an app never built has none yet.
    /// The root is resolved first: the ancestors of a relative `.` stop short of the workspace.
    fn read(app_root: &Path) -> Result<Self, (PathBuf, io::Error)> {
        let Ok(root) = app_root.canonicalize() else {
            return Ok(Self::default());
        };
        let Some(lock) = root
            .ancestors()
            .map(|directory| directory.join(LOCK_FILE))
            .find(|lock| lock.is_file())
        else {
            return Ok(Self::default());
        };
        fs::read_to_string(&lock)
            .map(|text| Self::parse(&text))
            .map_err(|error| (lock, error))
    }

    fn parse(text: &str) -> Self {
        let mut graph = Self::default();
        let mut package: Option<String> = None;
        let mut in_dependencies = false;
        for line in text.lines().map(str::trim) {
            if line == "[[package]]" {
                (package, in_dependencies) = (None, false);
            } else if let Some(name) = line.strip_prefix("name = ") {
                package = Some(name.trim_matches('"').to_owned());
            } else if line == "dependencies = [" {
                in_dependencies = true;
            } else if line == "]" {
                in_dependencies = false;
            } else if let Some(package) = package.as_ref().filter(|_| in_dependencies) {
                let dependency = line.trim_matches([' ', '"', ',']);
                let name = dependency.split(' ').next().unwrap_or(dependency);
                graph
                    .dependencies
                    .entry(package.clone())
                    .or_default()
                    .insert(name.to_owned());
            }
        }
        graph
    }

    /// The first package `wanted` accepts among those `from` depends on, through any package `entered` accepts.
    fn reaches<'graph>(
        &'graph self,
        from: &str,
        entered: impl Fn(&str) -> bool,
        wanted: impl Fn(&str) -> bool,
    ) -> Option<&'graph str> {
        let mut seen = BTreeSet::new();
        let mut pending: Vec<&str> = self.dependencies_of(from).collect();
        while let Some(package) = pending.pop() {
            if !seen.insert(package) {
                continue;
            }
            if wanted(package) {
                return Some(package);
            }
            if entered(package) {
                pending.extend(self.dependencies_of(package));
            }
        }
        None
    }

    fn dependencies_of(&self, package: &str) -> impl Iterator<Item = &str> {
        self.dependencies
            .get(package)
            .into_iter()
            .flatten()
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::{LockedGraph, declaration_line};

    #[test]
    fn a_dependency_is_found_only_in_dependency_tables() {
        let manifest = "[package]\nname = \"order\"\n\n[features]\nreqwest = [\"dep:reqwest\"]\n\n\
                        [dependencies]\nreqwest = { version = \"0.12\", optional = true }\n\n\
                        [dev-dependencies.ureq]\nversion = \"2\"\n";

        assert_eq!(declaration_line(manifest, "reqwest"), 8);
        assert_eq!(declaration_line(manifest, "ureq"), 10);
        assert_eq!(declaration_line(manifest, "hyper"), 1);
    }

    #[test]
    fn a_package_reaches_what_its_dependencies_pull_in() {
        let graph = LockedGraph::parse(
            "version = 4\n\n[[package]]\nname = \"stripe-sdk\"\nversion = \"1.0.0\"\n\
             dependencies = [\n \"serde\",\n \"http-layer 0.2.0\",\n]\n\n\
             [[package]]\nname = \"http-layer\"\nversion = \"0.2.0\"\n\
             dependencies = [\n \"reqwest 0.12.0 (registry+https://github.com/rust-lang/crates.io-index)\",\n]\n\n\
             [[package]]\nname = \"reqwest\"\nversion = \"0.12.0\"\n",
        );

        let reaches = |from: &str, entered: fn(&str) -> bool| {
            graph.reaches(from, entered, |package| package == "reqwest")
        };

        assert_eq!(reaches("stripe-sdk", |_| true), Some("reqwest"));
        assert_eq!(
            reaches("stripe-sdk", |package| package != "http-layer"),
            None
        );
        assert_eq!(reaches("serde", |_| true), None);
    }
}
