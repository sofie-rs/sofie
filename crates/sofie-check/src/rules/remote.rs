use sofie_model::naming::crate_name;
use sofie_model::{App, ContractTrait, Function, RemoteModule, TypeName};

use super::manifest::{declaration_line, manifest_text};
use super::{Finding, Rule};

pub(super) fn check_remote_modules(app: &App) -> Vec<Finding> {
    let mut findings = implementation_dependencies(app);
    findings.extend(local_contract_uses(app));
    findings
}

/// The implementation runs in its own app now; linking it here as well would run it twice.
fn implementation_dependencies(app: &App) -> Vec<Finding> {
    let (manifest, text) = manifest_text(&app.app);
    app.app
        .dependencies
        .iter()
        .filter_map(|dependency| {
            let remote = app
                .split
                .remote
                .iter()
                .find(|remote| remote.name == crate_name(&dependency.package))?;
            Some(Finding {
                rule: Rule::RemoteDependency,
                file: manifest.clone(),
                line: declaration_line(&text, &dependency.package),
                message: format!(
                    "the app depends on `{}`, but `[package.metadata.sofie] remote` serves module \
                     `{}` from another app",
                    dependency.package, remote.name
                ),
                fix: format!(
                    "remove `{}` and depend on `{}` only",
                    dependency.package, remote.contract.package
                ),
            })
        })
        .collect()
}

/// Only a trait whose every method crosses apps is provided remotely, so any other one is missing at run time.
fn local_contract_uses(app: &App) -> Vec<Finding> {
    let local_traits: Vec<(&RemoteModule, &ContractTrait)> = app
        .split
        .remote
        .iter()
        .flat_map(|remote| {
            remote
                .traits
                .iter()
                .filter(|contract_trait| !contract_trait.crosses_apps())
                .map(move |contract_trait| (remote, contract_trait))
        })
        .collect();
    app.modules
        .iter()
        .flat_map(|module| &module.code.functions)
        .flat_map(|function| {
            local_traits
                .iter()
                .filter(|(_, contract_trait)| takes_trait_object(function, &contract_trait.name))
                .map(|(remote, contract_trait)| {
                    local_contract_finding(function, remote, contract_trait)
                })
        })
        .collect()
}

fn takes_trait_object(function: &Function, trait_name: &str) -> bool {
    function
        .arguments
        .iter()
        .any(|argument| names_trait_object(argument, trait_name))
}

fn names_trait_object(ty: &TypeName, trait_name: &str) -> bool {
    let is_object = ty.name == "dyn" && ty.arguments.iter().any(|bound| bound.name == trait_name);
    is_object
        || ty
            .arguments
            .iter()
            .any(|argument| names_trait_object(argument, trait_name))
}

fn local_contract_finding(
    function: &Function,
    remote: &RemoteModule,
    contract_trait: &ContractTrait,
) -> Finding {
    let reasons: Vec<String> = contract_trait
        .methods
        .iter()
        .filter_map(|method| Some(format!("`{}` {}", method.name, method.stays_local?)))
        .collect();
    Finding {
        rule: Rule::RemoteCall,
        file: function.file.clone(),
        line: function.line,
        message: format!(
            "`{}` takes `dyn {}` of module `{}`, which runs in another app, but not every method \
             of it can cross apps: {}",
            function.path.join("::"),
            contract_trait.name,
            remote.name,
            reasons.join("; ")
        ),
        fix: "make each method `async`, take `&self` and owned or `&` arguments, and answer \
              `Result` of an owned type; or keep the module in this app"
            .to_owned(),
    }
}
