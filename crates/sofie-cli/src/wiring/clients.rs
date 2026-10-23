use proc_macro2::TokenStream;
use quote::quote;
use sofie_model::App;

use crate::clients::client_areas;

pub(super) fn clients_statement(app: &App) -> Option<TokenStream> {
    let bundles: Vec<TokenStream> = client_areas(app)
        .iter()
        .map(|area| {
            let name = &area.area;
            let keys = area.keys();
            quote!(sofie::engine::client_bundle!(#name, &[#(#keys),*]))
        })
        .collect();
    if bundles.is_empty() {
        return None;
    }
    Some(quote!(screens.clients(vec![#(#bundles),*]);))
}
