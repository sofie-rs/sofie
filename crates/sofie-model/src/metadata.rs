use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::naming::crate_name;
use crate::{Dependency, ModelError};

#[derive(Clone)]
pub(crate) struct Package {
    pub name: String,
    /// The lib target's name, which `[lib] name` may set apart from the package's.
    pub crate_name: String,
    pub dependencies: Vec<Dependency>,
    pub directory: PathBuf,
    pub serves: Vec<String>,
    pub remote: Vec<String>,
    pub structure_ceilings: bool,
}

/// Read through `cargo metadata` so each manifest is interpreted exactly as Cargo does.
pub(crate) struct Packages {
    manifest: PathBuf,
    by_manifest: BTreeMap<PathBuf, Package>,
    /// Resolving the whole dependency graph can be slow and need the network, so it is read once and only on demand.
    resolved: OnceCell<Value>,
}

#[derive(Clone, Copy)]
enum Graph {
    WorkspaceOnly,
    WithDependencies,
}

impl Packages {
    pub(crate) fn read(manifest: &Path) -> Result<Self, ModelError> {
        Ok(Self {
            manifest: manifest.to_path_buf(),
            by_manifest: packages_of(manifest, &cargo_metadata(manifest, Graph::WorkspaceOnly)?)?,
            resolved: OnceCell::new(),
        })
    }

    pub(crate) fn at(&self, manifest: &Path) -> Result<&Package, ModelError> {
        self.by_manifest.get(&canonical(manifest)?).ok_or_else(|| {
            ModelError::in_file(
                manifest,
                "`cargo metadata` did not list this package; make it a member of the app's workspace",
            )
        })
    }

    pub(crate) fn members(&self) -> impl Iterator<Item = &Package> {
        self.by_manifest.values()
    }

    pub(crate) fn dependency(&self, name: &str) -> Result<Option<Package>, ModelError> {
        if let Some(member) = named(&self.by_manifest, name) {
            return Ok(Some(member.clone()));
        }
        let resolved = packages_of(&self.manifest, self.resolved()?)?;
        Ok(named(&resolved, name).cloned())
    }

    pub(crate) fn enabled_features(&self, name: &str) -> Result<Vec<String>, ModelError> {
        Ok(enabled_features(self.resolved()?, name))
    }

    fn resolved(&self) -> Result<&Value, ModelError> {
        if let Some(resolved) = self.resolved.get() {
            return Ok(resolved);
        }
        let resolved = cargo_metadata(&self.manifest, Graph::WithDependencies)?;
        Ok(self.resolved.get_or_init(|| resolved))
    }
}

const FRAMEWORK_PACKAGE: &str = "sofie-framework";

/// Resolving them reads the whole dependency graph, so they are read only when asked.
pub fn framework_features(app_root: &Path) -> Result<Vec<String>, ModelError> {
    let manifest = app_root.join("app").join("Cargo.toml");
    let metadata = cargo_metadata(&manifest, Graph::WithDependencies)?;
    Ok(enabled_features(&metadata, FRAMEWORK_PACKAGE))
}

/// As Cargo resolved them for the whole workspace, so as they are compiled.
fn enabled_features(metadata: &Value, name: &str) -> Vec<String> {
    let id = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|package| package["name"] == name)
        .and_then(|package| package["id"].as_str());
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .into_iter()
        .flatten();
    nodes
        .filter(|node| id.is_some() && node["id"].as_str() == id)
        .flat_map(|node| node["features"].as_array().into_iter().flatten())
        .filter_map(|feature| feature.as_str().map(str::to_owned))
        .collect()
}

pub fn workspace_root(manifest: &Path) -> Result<PathBuf, ModelError> {
    let metadata = cargo_metadata(manifest, Graph::WorkspaceOnly)?;
    metadata["workspace_root"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| ModelError::in_file(manifest, "`cargo metadata` named no workspace root"))
}

fn named<'packages>(
    packages: &'packages BTreeMap<PathBuf, Package>,
    name: &str,
) -> Option<&'packages Package> {
    packages.values().find(|package| package.name == name)
}

fn packages_of(
    manifest: &Path,
    metadata: &Value,
) -> Result<BTreeMap<PathBuf, Package>, ModelError> {
    let listed = metadata["packages"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let lib_names: BTreeMap<&str, String> = listed
        .iter()
        .filter_map(|package| Some((package["name"].as_str()?, lib_name(package)?)))
        .collect();
    listed
        .iter()
        .map(|package| package_of(manifest, package, &lib_names))
        .collect()
}

fn cargo_metadata(manifest: &Path, graph: Graph) -> Result<Value, ModelError> {
    // Cargo sets `CARGO` when it runs us; prefer it so the same toolchain reads the manifest.
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command.args(["metadata", "--format-version", "1"]);
    if matches!(graph, Graph::WorkspaceOnly) {
        command.arg("--no-deps");
    }
    let output = command
        .arg("--manifest-path")
        .arg(manifest)
        .output()
        .map_err(|error| {
            ModelError::in_file(
                manifest,
                format!("running `cargo metadata` failed: {error}"),
            )
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ModelError::in_file(
            manifest,
            format!(
                "`cargo metadata` rejected the workspace; fix it until `cargo metadata` succeeds: {}",
                stderr.trim()
            ),
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| {
        ModelError::in_file(
            manifest,
            format!("`cargo metadata` printed invalid JSON: {error}"),
        )
    })
}

fn package_of(
    manifest: &Path,
    package: &Value,
    lib_names: &BTreeMap<&str, String>,
) -> Result<(PathBuf, Package), ModelError> {
    let incomplete = || {
        ModelError::in_file(
            manifest,
            "`cargo metadata` listed a package without a name or manifest path",
        )
    };
    let name = package["name"].as_str().ok_or_else(incomplete)?;
    let listed_manifest = canonical(Path::new(
        package["manifest_path"].as_str().ok_or_else(incomplete)?,
    ))?;
    let directory = listed_manifest
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let names = |key: &str| -> Vec<String> {
        package["metadata"]["sofie"][key]
            .as_array()
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .filter_map(|name| name.as_str().map(str::to_owned))
            .collect()
    };
    let package = Package {
        name: name.to_owned(),
        crate_name: lib_names
            .get(name)
            .cloned()
            .unwrap_or_else(|| crate_name(name)),
        dependencies: normal_dependencies(package, lib_names),
        directory,
        serves: names("serves"),
        remote: names("remote"),
        structure_ceilings: structure_ceilings(package, &listed_manifest)?,
    };
    Ok((listed_manifest, package))
}

fn structure_ceilings(package: &Value, manifest: &Path) -> Result<bool, ModelError> {
    match &package["metadata"]["sofie"]["structure-ceilings"] {
        Value::Null => Ok(false),
        Value::Bool(on) => Ok(*on),
        _ => Err(ModelError::in_file(
            manifest,
            "`[package.metadata.sofie] structure-ceilings` is `true` or `false`",
        )),
    }
}

/// `crate-type` lists these in place of `lib`.
const LIB_KINDS: [&str; 6] = ["lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"];

fn lib_name(package: &Value) -> Option<String> {
    let targets = package["targets"].as_array()?;
    let lib = targets.iter().find(|target| {
        target["kind"].as_array().is_some_and(|kinds| {
            kinds
                .iter()
                .any(|kind| LIB_KINDS.iter().any(|lib| kind == lib))
        })
    })?;
    lib["name"].as_str().map(crate_name)
}

fn canonical(manifest: &Path) -> Result<PathBuf, ModelError> {
    fs::canonicalize(manifest).map_err(|error| {
        ModelError::in_file(manifest, format!("cannot read the manifest: {error}"))
    })
}

/// A package `cargo metadata` did not list is named after itself: knowing its `[lib] name` needs the slow full graph.
fn normal_dependencies(package: &Value, lib_names: &BTreeMap<&str, String>) -> Vec<Dependency> {
    let dependencies = package["dependencies"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    dependencies
        .iter()
        .filter(|dependency| dependency["kind"].is_null())
        .filter_map(|dependency| {
            let package = dependency["name"].as_str()?;
            let crate_name = match dependency["rename"].as_str() {
                Some(rename) => crate_name(rename),
                None => lib_names
                    .get(package)
                    .cloned()
                    .unwrap_or_else(|| crate_name(package)),
            };
            Some(Dependency {
                package: package.to_owned(),
                crate_name,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::enabled_features;

    #[test]
    fn enabled_features_are_the_resolved_ones_of_the_named_package() {
        let metadata = json!({
            "packages": [
                {"name": "sofie-framework", "id": "framework 0.1"},
                {"name": "shop", "id": "shop 0.1"},
            ],
            "resolve": {"nodes": [
                {"id": "shop 0.1", "features": ["default"]},
                {"id": "framework 0.1", "features": ["engine", "i18n"]},
            ]},
        });

        assert_eq!(
            enabled_features(&metadata, "sofie-framework"),
            ["engine", "i18n"]
        );
        assert_eq!(
            enabled_features(&metadata, "sofie-identity"),
            Vec::<String>::new()
        );
    }
}
