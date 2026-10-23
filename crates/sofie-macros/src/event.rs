use proc_macro2::TokenStream;
use quote::quote;
use syn::DeriveInput;

use crate::input::input_impl;
use crate::serialize::{Reachable, serialize_impl};
use crate::shape::{named_fields, resolve_impl};

pub(crate) fn derive(input: &DeriveInput) -> syn::Result<TokenStream> {
    let fields = named_fields(input, "Event")?;
    let ident = &input.ident;
    let name = event_name(&ident.to_string());
    let read = input_impl(ident, &fields);
    let serialize = serialize_impl(ident, &fields, Reachable::Omitted);
    let resolve = resolve_impl(
        ident,
        &quote!(::sofie::db::Writes),
        &quote! {
            ::core::future::ready(
                context
                    .event(<Self as ::sofie::db::Event>::NAME)
                    .and_then(<Self as ::sofie::db::Input>::from_json),
            )
        },
    );
    Ok(quote! {
        impl ::sofie::db::Event for #ident {
            const NAME: &'static str = #name;
        }
        #read
        #serialize
        #resolve
    })
}

/// Named by module, not by contract crate, so moving an event keeps stored messages deliverable.
pub(crate) fn event_name(type_name: &str) -> String {
    let krate = std::env::var("CARGO_CRATE_NAME").unwrap_or_default();
    format!(
        "{}.{type_name}",
        krate.strip_suffix("_contract").unwrap_or(&krate)
    )
}
