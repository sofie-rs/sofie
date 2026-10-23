use proc_macro2::{Ident, TokenStream};
use quote::quote;

use crate::shape::NamedField;

/// Clients read `<field>_to` as the states `<field>` may move to.
pub(crate) const REACHABLE_SUFFIX: &str = "_to";

#[derive(Clone, Copy)]
pub(crate) enum Reachable {
    Included,
    Omitted,
}

pub(crate) fn serde_impls(ident: &Ident, module: &TokenStream, value: &TokenStream) -> TokenStream {
    quote! {
        impl ::sofie::serde::Serialize for #ident {
            fn serialize<SofieSerializer: ::sofie::serde::Serializer>(
                &self,
                serializer: SofieSerializer,
            ) -> ::core::result::Result<SofieSerializer::Ok, SofieSerializer::Error> {
                #module::serialize(#value, serializer)
            }
        }

        impl<'de> ::sofie::serde::Deserialize<'de> for #ident {
            fn deserialize<SofieDeserializer: ::sofie::serde::Deserializer<'de>>(
                deserializer: SofieDeserializer,
            ) -> ::core::result::Result<Self, SofieDeserializer::Error> {
                #module::deserialize(deserializer)
            }
        }
    }
}

pub(crate) fn serialize_impl(
    ident: &Ident,
    fields: &[NamedField],
    reachable: Reachable,
) -> TokenStream {
    let type_name = ident.to_string();
    let count = field_count(fields, reachable, 0);
    let entries = fields.iter().map(|field| entry(field, reachable));
    quote! {
        impl ::sofie::serde::Serialize for #ident {
            fn serialize<SofieSerializer: ::sofie::serde::Serializer>(
                &self,
                serializer: SofieSerializer,
            ) -> ::core::result::Result<SofieSerializer::Ok, SofieSerializer::Error> {
                use ::sofie::serde::ser::SerializeStruct as _;
                let mut state = serializer.serialize_struct(#type_name, #count)?;
                #(#entries)*
                state.end()
            }
        }
    }
}

/// Length-prefixed formats write this count, so it adds each `<field>_to` [`entry`] writes.
fn field_count(fields: &[NamedField], reachable: Reachable, others: usize) -> TokenStream {
    let count = fields.len() + others;
    let Reachable::Included = reachable else {
        return quote!(#count);
    };
    let members = fields.iter().map(|field| field.ident);
    quote! {
        #count #(+ usize::from(::sofie::core::moves::targets(&self.#members).is_some()))*
    }
}

pub(crate) fn entry(field: &NamedField, reachable: Reachable) -> TokenStream {
    let name = field.name();
    let member = field.ident;
    let value = quote! { state.serialize_field(#name, &self.#member)?; };
    let Reachable::Included = reachable else {
        return value;
    };
    let reachable_name = format!("{name}{REACHABLE_SUFFIX}");
    quote! {
        #value
        if let ::core::option::Option::Some(targets) = ::sofie::core::moves::targets(&self.#member) {
            state.serialize_field(#reachable_name, targets)?;
        }
    }
}

/// Named and ordered as [`serialize_impl`] writes them, so a screen reads the fields JSON carries.
pub(crate) fn record_impl(
    ident: &Ident,
    fields: &[NamedField],
    reachable: Reachable,
) -> TokenStream {
    let (names, views): (Vec<String>, Vec<TokenStream>) = fields
        .iter()
        .flat_map(|field| {
            let member = field.ident;
            let shown =
                quote!(::core::option::Option::Some(::sofie::core::Show::show(&self.#member)));
            let moves = quote! {
                ::sofie::core::moves::targets(&self.#member)
                    .map(|targets| ::sofie::core::Shown::Record(targets))
            };
            let reachable_name = format!("{}{REACHABLE_SUFFIX}", field.name());
            let moves = matches!(reachable, Reachable::Included).then_some((reachable_name, moves));
            std::iter::once((field.name(), shown)).chain(moves)
        })
        .unzip();
    quote! {
        impl ::sofie::core::show::Fields for #ident {
            fn name(&self, index: usize) -> ::core::option::Option<&str> {
                [#(#names),*].get(index).copied()
            }

            fn get(&self, name: &str) -> ::core::option::Option<::sofie::core::Shown<'_>> {
                match name { #(#names => #views,)* _ => ::core::option::Option::None }
            }
        }

        impl ::sofie::core::Show for #ident {
            fn show(&self) -> ::sofie::core::Shown<'_> { ::sofie::core::Shown::Record(self) }
        }
    }
}

#[cfg(test)]
mod tests {
    use syn::DeriveInput;

    use super::{Reachable, serialize_impl};
    use crate::shape::named_fields;

    #[test]
    fn the_declared_field_count_includes_each_written_move_list() {
        let input: DeriveInput = syn::parse_quote! {
            struct Row { status: Status, quantity: i64 }
        };
        let fields = named_fields(&input, "ReadModel").unwrap();
        let expanded = serialize_impl(&input.ident, &fields, Reachable::Included).to_string();
        let declared = expanded
            .split("serialize_struct")
            .nth(1)
            .and_then(|call| call.split(") ?").next())
            .unwrap();
        assert!(declared.contains("is_some"), "{declared}");
        assert!(declared.contains("self . status"), "{declared}");
    }
}
