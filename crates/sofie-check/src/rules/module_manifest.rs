use sofie_model::{Manifest, Module, ModuleConfig};

use super::{Finding, Rule};

const DECLARATION: &str = "`pub const MODULE: sofie::Module = sofie::Module { name: \"…\", tier: \
                           sofie::Tier::Core, context: None, settings: &[], \
                           ..crate::wiring::GENERATED };`";

pub(super) fn check_module_manifests(modules: &[Module]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for module in modules {
        match &module.manifest {
            None => findings.push(missing(module)),
            Some(manifest) => {
                findings.extend(check_manifest(module, manifest));
                findings.extend(duplicate_setting(manifest));
            }
        }
        findings.extend(module.config.as_ref().and_then(undeclared_config));
    }
    findings
}

fn duplicate_setting(manifest: &Manifest) -> Option<Finding> {
    let (index, name) = manifest
        .settings
        .iter()
        .enumerate()
        .find(|(index, name)| manifest.settings[..*index].contains(name))?;
    Some(Finding {
        rule: Rule::ModuleManifest,
        file: manifest.file.clone(),
        line: manifest.line,
        message: format!(
            "setting `{name}` is declared twice (setting {} of the manifest), so nobody changing it \
             could tell which one",
            index + 1
        ),
        fix: format!("rename or remove one of the settings named `{name}`"),
    })
}

/// The wiring loads the config by its path, so it must be public.
fn undeclared_config(config: &ModuleConfig) -> Option<Finding> {
    (!config.declared).then(|| Finding {
        rule: Rule::ModuleManifest,
        file: config.file.clone(),
        line: config.line,
        message: "no `mod` declaration reaches src/config.rs, so the wiring cannot load it"
            .to_owned(),
        fix: "add `pub mod config;` to src/lib.rs".to_owned(),
    })
}

fn check_manifest(module: &Module, manifest: &Manifest) -> Option<Finding> {
    let finding = |message: String, fix: String| Finding {
        rule: Rule::ModuleManifest,
        file: manifest.file.clone(),
        line: manifest.line,
        message,
        fix,
    };
    if manifest.name != module.krate.crate_name {
        return Some(finding(
            format!(
                "the manifest names the module `{}` but its crate is `{}`, and the name is its \
                 schema and configuration prefix",
                manifest.name, module.krate.crate_name
            ),
            format!("write `name: \"{}\"`", module.krate.crate_name),
        ));
    }
    if !manifest.declared {
        return Some(finding(
            "no `mod` declaration reaches the manifest, so the compiler never checks it".to_owned(),
            "add `pub mod module;` to src/lib.rs".to_owned(),
        ));
    }
    None
}

fn missing(module: &Module) -> Finding {
    Finding {
        rule: Rule::ModuleManifest,
        file: module.krate.path.join("Cargo.toml"),
        line: 1,
        message: format!(
            "module `{}` has no src/module.rs, so its tier and the rules that follow from it are \
             unknown",
            module.krate.package
        ),
        fix: format!(
            "create src/module.rs declaring {DECLARATION}, and `pub mod module;` and `mod wiring;` \
             in src/lib.rs"
        ),
    }
}
