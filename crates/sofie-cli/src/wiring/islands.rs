use proc_macro2::TokenStream;
use quote::quote;
use sofie_model::{App, island_bundles};

/// Hashed at startup rather than in a constant: compile-time evaluation over a bundle's bytes is too slow.
pub(super) fn islands_statement(app: &App) -> Option<TokenStream> {
    let bundles: Vec<TokenStream> = island_bundles(&app.modules)
        .iter()
        .map(|bundle| {
            let area = &bundle.area;
            quote!(sofie::engine::island_bundle!(#area))
        })
        .collect();
    if bundles.is_empty() {
        return None;
    }
    Some(quote!(screens.islands(vec![#(#bundles),*]);))
}
