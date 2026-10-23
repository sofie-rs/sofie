use proc_macro2::{Ident, TokenStream};
use quote::quote;

use crate::serialize::{Reachable, entry};
use crate::shape::NamedField;

/// Higher-ranked bound: an aggregate with an unshowable field (a password hash) still compiles.
pub(crate) fn serialize_impl(ident: &Ident, fields: &[NamedField]) -> TokenStream {
    let fields: Vec<NamedField> = fields
        .iter()
        .filter(|field| !field.has_attribute("sensitive"))
        .copied()
        .collect();
    let type_name = ident.to_string();
    let types: Vec<_> = fields.iter().map(|field| field.ty).collect();
    let counts = fields.iter().map(|field| {
        let member = field.ident;
        quote!(1 + usize::from(::sofie::core::moves::targets(&self.#member).is_some()))
    });
    let entries = fields.iter().map(|field| entry(field, Reachable::Included));
    let count = quote! {
        1 #(+ if <#types as ::sofie::core::Cast>::SENSITIVE { 0 } else { #counts })*
    };
    let entries = quote! {
        #(if !<#types as ::sofie::core::Cast>::SENSITIVE { #entries })*
    };
    quote! {
        impl ::sofie::serde::Serialize for #ident
        where
            #(for<'sofie> #types: ::sofie::serde::Serialize,)*
        {
            fn serialize<SofieSerializer: ::sofie::serde::Serializer>(
                &self,
                serializer: SofieSerializer,
            ) -> ::core::result::Result<SofieSerializer::Ok, SofieSerializer::Error> {
                use ::sofie::serde::ser::SerializeStruct as _;
                let mut state = serializer.serialize_struct(#type_name, #count)?;
                state.serialize_field("id", &::sofie::db::Aggregate::id(self))?;
                #entries
                state.end()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use syn::DeriveInput;

    use super::serialize_impl;
    use crate::shape::named_fields;

    #[test]
    fn a_sensitive_field_is_left_out_of_the_json() {
        let input: DeriveInput = syn::parse_quote! {
            struct Payer { name: String, #[sensitive] tax_number: String }
        };
        let fields = named_fields(&input, "Aggregate").unwrap();
        let expanded = serialize_impl(&input.ident, &fields).to_string();
        assert!(expanded.contains("\"name\""), "{expanded}");
        assert!(!expanded.contains("tax_number"), "{expanded}");
    }

    #[test]
    fn a_field_of_a_sensitive_type_is_left_out_of_the_json_and_its_count() {
        let input: DeriveInput = syn::parse_quote! {
            struct Payer { name: String, iban: Iban }
        };
        let fields = named_fields(&input, "Aggregate").unwrap();
        let expanded = serialize_impl(&input.ident, &fields).to_string();
        let skipped = "if ! < Iban as :: sofie :: core :: Cast > :: SENSITIVE {";
        let declared = expanded.split("serialize_struct").nth(1).unwrap();
        let declared = declared.split(") ?").next().unwrap();
        assert!(expanded.contains(skipped), "{expanded}");
        assert!(
            declared.contains("< Iban as :: sofie :: core :: Cast > :: SENSITIVE"),
            "{declared}"
        );
    }
}
