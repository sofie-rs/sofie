use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Attribute, Expr};

use crate::shape::NamedField;

/// `min`/`max` bound text characters or a number's value; `length`/`range` say which.
pub(crate) const ATTRIBUTES: [&str; 6] = ["min", "max", "length", "range", "required", "unique"];

pub(crate) fn constraints(field: &NamedField) -> syn::Result<Option<TokenStream>> {
    let mut constraints = Vec::new();
    for attribute in field.attrs {
        let Some(name) = ATTRIBUTES
            .into_iter()
            .find(|name| attribute.path().is_ident(name))
        else {
            continue;
        };
        constraints.extend(constraints_of(field, name, attribute)?);
    }
    if constraints.is_empty() {
        return Ok(None);
    }
    Ok(Some(quote! { const { &[#(#constraints),*] } }))
}

fn path_only(attribute: &Attribute, variant: &TokenStream) -> syn::Result<Vec<TokenStream>> {
    attribute.meta.require_path_only()?;
    Ok(vec![quote!(::sofie::core::Constraint::#variant)])
}

fn constraints_of(
    field: &NamedField,
    name: &str,
    attribute: &Attribute,
) -> syn::Result<Vec<TokenStream>> {
    let ty = field.ty;
    let text = quote_spanned!(ty.span() => <#ty as ::sofie::core::Measured>::TEXT);
    match name {
        "required" => path_only(attribute, &quote!(Required)),
        "unique" => path_only(attribute, &quote!(Unique)),
        "min" => Ok(vec![by_kind(&text, "Min", &attribute.parse_args()?)]),
        "max" => Ok(vec![by_kind(&text, "Max", &attribute.parse_args()?)]),
        _ => bounds(attribute, &text, name == "length"),
    }
}

fn by_kind(text: &TokenStream, bound_name: &str, bound: &Expr) -> TokenStream {
    let length = format_ident!("{bound_name}Length");
    let value = format_ident!("{bound_name}");
    quote! {
        if #text {
            ::sofie::core::Constraint::#length(#bound)
        } else {
            ::sofie::core::Constraint::#value(#bound)
        }
    }
}

fn bounds(
    attribute: &Attribute,
    measured: &TokenStream,
    text: bool,
) -> syn::Result<Vec<TokenStream>> {
    let (message, kind, unit) = if text {
        let message = "`#[length]` bounds text; use `#[range]` for a number";
        (message, quote!(#measured), "Length")
    } else {
        let message = "`#[range]` bounds a number; use `#[length]` for text";
        (message, quote!(!#measured), "")
    };
    let mut constraints = Vec::new();
    attribute.parse_nested_meta(|bound| {
        let bound_name = if bound.path.is_ident("min") {
            "Min"
        } else if bound.path.is_ident("max") {
            "Max"
        } else {
            return Err(bound.error("expected `min = …` or `max = …`"));
        };
        let variant = format_ident!("{bound_name}{unit}", span = bound.path.span());
        let value: Expr = bound.value()?.parse()?;
        // At the attribute, so a misused bound is reported where it is written.
        constraints.push(quote_spanned! {attribute.span()=> {
            ::core::assert!(#kind, #message);
            ::sofie::core::Constraint::#variant(#value)
        }});
        Ok(())
    })?;
    Ok(constraints)
}
