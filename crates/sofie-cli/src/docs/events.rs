use sofie_model::{App, Effects};

use crate::sorted;

pub(super) fn raised_by(app: &App, event: &str) -> Vec<String> {
    let raises = |effects: &Effects| {
        effects
            .raised
            .iter()
            .any(|raised| raised.event.as_deref() == Some(event))
    };
    let mut found = Vec::new();
    for module in sorted::modules(app) {
        let name = &module.krate.crate_name;
        found.extend(
            module
                .code
                .functions
                .iter()
                .filter(|function| raises(&function.effects))
                .map(|function| format!("{name}::{}", function.path.join("::"))),
        );
        found.extend(
            module
                .code
                .methods
                .iter()
                .filter(|method| raises(&method.effects))
                .map(|method| format!("{name}::{}::{}", method.owner, method.name)),
        );
    }
    found
}

pub(super) fn heard_by(app: &App, event: &str) -> Vec<String> {
    sorted::modules(app)
        .into_iter()
        .flat_map(|module| {
            module
                .listeners
                .iter()
                .filter(|listener| listener.event.last().is_some_and(|heard| heard == event))
                .map(|listener| {
                    format!(
                        "{}::{}",
                        module.krate.crate_name,
                        listener.function.join("::")
                    )
                })
        })
        .collect()
}
