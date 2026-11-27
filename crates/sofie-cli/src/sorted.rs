//! By name or first sight, so what is generated or reported does not depend on the order the
//! model was read in.

use proc_macro2::TokenStream;
use sofie_model::{App, AreaContribution, Module};

pub(crate) fn modules(app: &App) -> Vec<&Module> {
    let mut modules: Vec<&Module> = app.modules.iter().collect();
    modules.sort_by(|left, right| left.krate.crate_name.cmp(&right.krate.crate_name));
    modules
}

/// By rendered text, for generated lists whose order means nothing.
pub(crate) fn by_text(items: impl IntoIterator<Item = TokenStream>) -> Vec<TokenStream> {
    let mut keyed: Vec<(String, TokenStream)> = items
        .into_iter()
        .map(|item| (item.to_string(), item))
        .collect();
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    keyed.into_iter().map(|(_, item)| item).collect()
}

pub(crate) fn first_seen<T: PartialEq>(items: impl IntoIterator<Item = T>) -> Vec<T> {
    let mut seen = Vec::new();
    for item in items {
        if !seen.contains(&item) {
            seen.push(item);
        }
    }
    seen
}

pub(crate) fn areas(module: &Module) -> Vec<&AreaContribution> {
    let mut areas: Vec<&AreaContribution> = module.areas.iter().collect();
    areas.sort_by(|left, right| left.area.cmp(&right.area));
    areas
}
