use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sofie_model::Module;

use super::{Finding, Rule};

/// The derive takes the `factory.rs` in an aggregate's module folder as that aggregate's
/// constructor, so a second aggregate there silently loses its default factory.
pub(super) fn check_aggregate_folders(modules: &[Module]) -> Vec<Finding> {
    modules.iter().flat_map(shared_folders).collect()
}

fn shared_folders(module: &Module) -> Vec<Finding> {
    let mut by_folder: BTreeMap<PathBuf, Vec<&sofie_model::Aggregate>> = BTreeMap::new();
    for aggregate in &module.code.aggregates {
        by_folder
            .entry(module_folder(&aggregate.file))
            .or_default()
            .push(aggregate);
    }
    by_folder
        .values()
        .flat_map(|aggregates| {
            let first = &aggregates[0].name;
            aggregates[1..].iter().map(move |aggregate| Finding {
                rule: Rule::AggregateFolder,
                file: aggregate.file.clone(),
                line: aggregate.line,
                message: format!(
                    "`{}` shares a folder with `{first}`, so a `factory.rs` there is taken as both \
                     aggregates' constructor and the derive writes neither a default one",
                    aggregate.name
                ),
                fix: format!(
                    "move `{}` to its own folder, `src/{}/mod.rs`",
                    aggregate.name,
                    sofie_model::naming::snake_case(&aggregate.name)
                ),
            })
        })
        .collect()
}

/// As the derive reads it: `x/mod.rs` and `lib.rs` are their folder, `x.rs` is `x/`.
fn module_folder(file: &Path) -> PathBuf {
    let is_root = file
        .file_stem()
        .is_some_and(|stem| stem == "mod" || stem == "lib" || stem == "main");
    match file.parent() {
        Some(parent) if is_root => parent.to_path_buf(),
        _ => file.with_extension(""),
    }
}
