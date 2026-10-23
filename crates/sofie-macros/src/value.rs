use proc_macro2::{Ident, TokenStream};
use quote::quote;
use syn::{Attribute, DeriveInput, Fields, Member, Type};

use crate::case::snake_case;
use crate::input::{local, take};
use crate::serialize::{Reachable, record_impl, serde_impls, serialize_impl};
use crate::shape::{
    NamedField, attributes_named, named_fields, reject_generics, resolve_impl, struct_fields,
};

pub(crate) fn derive(input: &DeriveInput) -> syn::Result<TokenStream> {
    reject_generics(input, "Value")?;
    let ident = &input.ident;
    let fields = struct_fields(input, "Value")?;
    let sensitive = attributes_named(&input.attrs, "sensitive").next();
    let value = match single_field(fields) {
        Some((member, ty)) => single_field_impls(ident, &member, ty, sensitive.is_some()),
        None => several_fields_impls(input, fields, sensitive.is_some())?,
    };
    let debug = masked_debug(ident, sensitive)?;
    let input = lone_input_impls(ident);
    Ok(quote! { #value #debug #input })
}

fn lone_input_impls(ident: &Ident) -> TokenStream {
    let field = snake_case(&ident.to_string());
    let resolve = resolve_impl(
        ident,
        &quote!(::sofie::db::Writes),
        &quote!(::core::future::ready(
            context.input().and_then(::sofie::db::lone_value)
        )),
    );
    quote! {
        impl ::sofie::db::InputValue for #ident {
            const FIELD: &'static str = #field;
        }
        #resolve
    }
}

fn single_field(fields: &Fields) -> Option<(Member, &Type)> {
    let mut pairs = fields.members().zip(fields.iter().map(|field| &field.ty));
    match (pairs.next(), pairs.next()) {
        (Some(pair), None) => Some(pair),
        _ => None,
    }
}

fn single_field_impls(ident: &Ident, member: &Member, ty: &Type, sensitive: bool) -> TokenStream {
    let serde = serde_impls(ident, &quote!(::sofie::core::value), &quote!(self));
    quote! {
        impl ::sofie::core::Value for #ident {
            type Inner = #ty;
            const SENSITIVE: bool = #sensitive;

            fn inner(&self) -> &#ty {
                &self.#member
            }

            fn try_from_inner(
                inner: #ty,
            ) -> ::core::result::Result<Self, impl ::core::fmt::Display> {
                Self::try_new(inner)
            }
        }

        #serde

        impl ::sofie::values::SurfacesFromInner for #ident {
            const SENSITIVE: bool = #sensitive;
        }
    }
}

/// One column per field, `<field>_<member>`; `try_new` takes fields in declaration order.
fn several_fields_impls(
    input: &DeriveInput,
    fields: &Fields,
    sensitive: bool,
) -> syn::Result<TokenStream> {
    if !matches!(fields, Fields::Named(_)) || fields.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "a value wraps one field, or several named fields",
        ));
    }
    let fields = named_fields(input, "Value")?;
    let ident = &input.ident;
    let names: Vec<String> = fields.iter().map(NamedField::name).collect();
    let suffixes = names.iter().map(|name| format!("_{name}"));
    let orders = (0..names.len()).map(|index| index == 0);
    let members: Vec<&Ident> = fields.iter().map(|field| field.ident).collect();
    let types: Vec<&Type> = fields.iter().map(|field| field.ty).collect();
    let locals: Vec<Ident> = fields.iter().map(local).collect();
    let reads = fields.iter().map(take);
    let serialize = serialize_impl(ident, &fields, Reachable::Omitted);
    let record = record_impl(ident, &fields, Reachable::Omitted);
    Ok(quote! {
        #record

        impl ::sofie::core::Cast for #ident {
            const COLUMNS: &'static [::sofie::core::ColumnSpec] = &[#(::sofie::core::ColumnSpec {
                suffix: #suffixes,
                ty: ::sofie::core::value::single_column(<#types as ::sofie::core::Cast>::COLUMNS),
                orders: #orders,
                values: &[],
            }),*];
            const NULLABLE: bool = true #(&& <#types as ::sofie::core::Cast>::NULLABLE)*;
            const SENSITIVE: bool = #sensitive #(|| <#types as ::sofie::core::Cast>::SENSITIVE)*;
            const HOLDS_STATES: bool = false #(|| <#types as ::sofie::core::Cast>::HOLDS_STATES)*;

            fn encode(&self, out: &mut ::std::vec::Vec<::sofie::core::SqlValue>) {
                #(::sofie::core::Cast::encode(&self.#members, out);)*
            }

            fn decode(row: &mut dyn ::sofie::core::ColumnReader) -> ::sofie::core::Result<Self> {
                #(let #locals = ::sofie::core::Cast::decode(row)?;)*
                ::sofie::core::value::stored(Self::try_new(#(#locals),*))
            }

            fn refuse_unlisted_move(
                field: &str,
                stored: &Self,
                current: &Self,
            ) -> ::sofie::core::Result<()> {
                #(::sofie::core::Cast::refuse_unlisted_move(field, &stored.#members, &current.#members)?;)*
                ::sofie::core::Result::Ok(())
            }
        }

        #serialize

        impl<'de> ::sofie::serde::Deserialize<'de> for #ident {
            fn deserialize<SofieDeserializer: ::sofie::serde::Deserializer<'de>>(
                deserializer: SofieDeserializer,
            ) -> ::core::result::Result<Self, SofieDeserializer::Error> {
                ::sofie::db::deserialize_fields(deserializer, |fields| {
                    #(let #locals = #reads;)*
                    ::core::result::Result::Ok(Self::try_new(#(#locals?),*))
                })
            }
        }
    })
}

fn masked_debug(ident: &Ident, sensitive: Option<&Attribute>) -> syn::Result<TokenStream> {
    let Some(attribute) = sensitive else {
        return Ok(TokenStream::new());
    };
    attribute.meta.require_path_only()?;
    let name = ident.to_string();
    Ok(quote! {
        impl ::core::fmt::Debug for #ident {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::sofie::core::value::write_masked(formatter, #name)
            }
        }
    })
}
