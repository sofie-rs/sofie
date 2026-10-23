use proc_macro2::{Ident, TokenStream};
use quote::quote;

use crate::shape::{NamedField, names_with};

/// With a `#[sensitive]` field the derive writes `Debug` itself, so a derived one cannot print
/// the field; a second `#[derive(Debug)]` is refused as a conflicting impl.
pub(crate) fn items(ident: &Ident, fields: &[NamedField]) -> TokenStream {
    let sensitive: Vec<String> = names_with(fields, "sensitive").collect();
    if sensitive.is_empty() {
        return TokenStream::new();
    }
    let shown = fields.iter().map(|field| {
        let name = field.name();
        let member = field.ident;
        if field.has_attribute("sensitive") {
            return quote!(.field(#name, &::core::format_args!("***")));
        }
        quote!(.field(#name, &self.#member))
    });
    #[cfg(feature = "log-fields")]
    let logged = quote! {
        impl ::sofie::values::SensitiveFields for #ident {
            const SENSITIVE_FIELDS: &'static [&'static str] = &[#(#sensitive),*];
        }
    };
    #[cfg(not(feature = "log-fields"))]
    let logged = TokenStream::new();
    quote! {
        impl ::core::fmt::Debug for #ident {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.debug_struct(::core::stringify!(#ident)) #(#shown)* .finish()
            }
        }
        #logged
    }
}
