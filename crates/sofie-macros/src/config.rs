use proc_macro2::TokenStream;
use quote::quote;
use syn::DeriveInput;

use crate::shape::{NamedField, named_fields, wrapped_type};

pub(crate) fn derive(input: &DeriveInput) -> syn::Result<TokenStream> {
    let ident = &input.ident;
    let fields = named_fields(input, "Config")?;
    let specs = fields.iter().map(field_spec);
    let members = fields.iter().map(|field| field.ident);
    let indexes = 0..fields.len();
    Ok(quote! {
        impl ::sofie::config::Group for #ident {
            const FIELDS: &'static [::sofie::config::FieldSpec] = &[#(#specs),*];

            fn build(
                entry: &::sofie::config::Entry<'_>,
            ) -> ::core::result::Result<Self, ::sofie::config::ConfigError> {
                ::core::result::Result::Ok(Self {
                    #(#members: entry.read(&Self::FIELDS[#indexes])?),*
                })
            }
        }

        impl ::sofie::config::Loaded for #ident {
            fn loaded() -> &'static ::std::sync::OnceLock<Self> {
                static LOADED: ::std::sync::OnceLock<#ident> = ::std::sync::OnceLock::new();
                &LOADED
            }
        }
    })
}

fn field_spec(field: &NamedField) -> TokenStream {
    let name = field.name();
    let key = name.to_ascii_uppercase();
    let inner = wrapped_type(field.ty, "Option");
    let optional = inner.is_some();
    let ty = inner.unwrap_or(field.ty);
    quote! {
        ::sofie::config::FieldSpec {
            name: #name,
            key: #key,
            optional: #optional,
            kind: <#ty as ::sofie::config::Field>::KIND,
            fallback: <#ty as ::sofie::config::Field>::FALLBACK,
        }
    }
}
