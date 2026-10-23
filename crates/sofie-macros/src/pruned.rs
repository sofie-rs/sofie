use proc_macro2::TokenStream;
use quote::quote;
use syn::DeriveInput;

/// Names the `<Aggregate>Pruned` that `sofie gen` writes into the module's contract.
pub(crate) fn pruned_constant(input: &DeriveInput) -> TokenStream {
    let name = crate::event::event_name(&format!("{}Pruned", input.ident));
    quote! {
        const PRUNED: ::core::option::Option<&'static str> = ::core::option::Option::Some(#name);
    }
}

#[cfg(test)]
mod tests {
    use super::pruned_constant;

    #[test]
    fn an_aggregate_names_the_pruned_event_its_contract_defines() {
        let input: syn::DeriveInput = syn::parse_quote! {
            pub struct Booking { meta: Meta, starts_at: Instant }
        };

        let constant = pruned_constant(&input).to_string();

        assert!(constant.contains(".BookingPruned\")"), "{constant}");
        assert!(!constant.contains("struct"), "{constant}");
    }
}
