use proc_macro2::TokenStream;
use quote::quote;
use sofie_model::App;

use crate::clients::client_areas;
use crate::sorted;

pub(super) fn clients_statement(app: &App) -> Option<TokenStream> {
    let bundles = sorted::by_text(client_areas(app).iter().map(|area| {
        let name = &area.area;
        let mut keys: Vec<&str> = area.keys().collect();
        keys.sort_unstable();
        quote!(sofie::engine::client_bundle!(#name, &[#(#keys),*]))
    }));
    if bundles.is_empty() {
        return None;
    }
    Some(quote!(screens.clients(vec![#(#bundles),*]);))
}
