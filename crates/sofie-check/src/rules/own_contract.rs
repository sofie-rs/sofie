use std::path::Path;

use sofie_model::{Effects, Function, Module, TypeName};

use super::{Finding, Rule};

/// A module's own contract is built from the database with full reach, so reading through it
/// from an owned area skips the owner filter, unaudited and racing concurrent writes.
pub(super) fn check_own_contract(modules: &[Module]) -> Vec<Finding> {
    modules
        .iter()
        .flat_map(|module| {
            entry_points(module)
                .filter_map(|path| module.code.function(path))
                .filter_map(|function| reading_own_contract(module, function))
                .chain(hand_built_checks(module))
        })
        .collect()
}

/// The derive's `across` is crate-private; its constructor, called by hand, reaches any module.
fn hand_built_checks(module: &Module) -> impl Iterator<Item = Finding> + '_ {
    let functions = module.code.functions.iter().map(|function| {
        let caller = function.path.join("::");
        hand_built(&caller, &function.effects, &function.file)
    });
    let methods = module.code.methods.iter().map(|method| {
        let caller = format!("{}::{}", method.owner, method.name);
        hand_built(&caller, &method.effects, &method.file)
    });
    functions.chain(methods).flatten()
}

fn hand_built(caller: &str, effects: &Effects, file: &Path) -> Option<Finding> {
    let call = effects.calls.iter().find(|call| {
        call.path
            .last()
            .is_some_and(|name| name == "for_derived_across_only")
    })?;
    Some(Finding {
        rule: Rule::OwnContract,
        file: file.to_path_buf(),
        line: call.line,
        message: format!(
            "`{caller}` builds a check across owners by hand, which reaches other modules' records"
        ),
        fix: FIX.to_owned(),
    })
}

fn entry_points(module: &Module) -> impl Iterator<Item = &Vec<String>> {
    let handlers = module
        .areas
        .iter()
        .flat_map(|area| &area.routes)
        .map(|route| &route.handler);
    let jobs = module.jobs.iter().map(|job| &job.function);
    let listeners = module.listeners.iter().map(|listener| &listener.function);
    handlers.chain(jobs).chain(listeners)
}

fn reading_own_contract(module: &Module, function: &Function) -> Option<Finding> {
    let contract = function
        .arguments
        .iter()
        .find_map(|argument| own_contract_trait(module, argument))?;
    Some(Finding {
        rule: Rule::OwnContract,
        file: function.file.clone(),
        line: function.line,
        message: format!(
            "`{}` takes `Dep<dyn {contract}>` and so reads through its own contract, which skips \
             the owner filter",
            function.path.join("::")
        ),
        fix: FIX.to_owned(),
    })
}

const FIX: &str = "check across owners with `<Aggregate>::across(&read)` and return its \
                   `.guard(...)` with the outcome, so the commit checks again";

fn own_contract_trait<'m>(module: &'m Module, argument: &TypeName) -> Option<&'m str> {
    let [object] = argument.arguments.as_slice() else {
        return None;
    };
    if argument.name != "Dep" || object.name != "dyn" {
        return None;
    }
    module
        .contract_traits
        .iter()
        .find(|contract| {
            object
                .arguments
                .iter()
                .any(|bound| bound.name == contract.name)
        })
        .map(|contract| contract.name.as_str())
}
