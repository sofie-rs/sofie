use proc_macro2::{Ident, TokenStream};
use quote::{quote, quote_spanned};
use syn::ext::IdentExt;
use syn::punctuated::Punctuated;
use syn::{Data, DeriveInput, Fields, Token, Variant};

use crate::case::screaming_snake_case;
use crate::serialize::serde_impls;
use crate::shape::{attributes_named, reject_generics};

struct State<'a> {
    ident: &'a Ident,
    name: String,
    next: Option<Vec<Ident>>,
}

pub(crate) fn derive(input: &DeriveInput) -> syn::Result<TokenStream> {
    reject_generics(input, "Enum")?;
    let Data::Enum(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "`#[derive(Enum)]` applies to enums only",
        ));
    };
    let states = data
        .variants
        .iter()
        .map(state)
        .collect::<syn::Result<Vec<_>>>()?;
    check_names(&states)?;
    check_targets(&states)?;
    let ident = &input.ident;
    let variants = variants_impl(ident, &states);
    let conversions = conversion_impls(ident, &states);
    let present = present_impl(ident, &states);
    #[cfg(feature = "fixtures")]
    let present = {
        let fixture = crate::fixture::enum_fixture(ident);
        quote!(#present #fixture)
    };
    Ok(quote! { #variants #conversions #present })
}

fn state(variant: &Variant) -> syn::Result<State<'_>> {
    if !matches!(variant.fields, Fields::Unit) {
        return Err(syn::Error::new_spanned(
            variant,
            "`#[derive(Enum)]` supports unit variants only",
        ));
    }
    check_tone(variant)?;
    Ok(State {
        ident: &variant.ident,
        name: screaming_snake_case(&variant.ident.unraw().to_string()),
        next: transition_targets(variant)?,
    })
}

fn check_tone(variant: &Variant) -> syn::Result<()> {
    for attribute in attributes_named(&variant.attrs, "tone") {
        let tone: Ident = attribute.parse_args()?;
        if !TONES.contains(&tone.to_string().as_str()) {
            return Err(syn::Error::new_spanned(
                tone,
                format!("a tone is one of {}", TONES.join(", ")),
            ));
        }
    }
    Ok(())
}

const TONES: [&str; 5] = ["neutral", "info", "success", "warning", "danger"];

fn transition_targets(variant: &Variant) -> syn::Result<Option<Vec<Ident>>> {
    let mut attributes = attributes_named(&variant.attrs, "to");
    let Some(attribute) = attributes.next() else {
        return Ok(None);
    };
    if let Some(repeated) = attributes.next() {
        return Err(syn::Error::new_spanned(
            repeated,
            "list every transition in one `#[to(…)]`",
        ));
    }
    let targets = attribute.parse_args_with(Punctuated::<Ident, Token![,]>::parse_terminated)?;
    if targets.is_empty() {
        return Err(syn::Error::new_spanned(
            attribute,
            "`#[to(…)]` needs at least one variant; leave it out for a terminal state",
        ));
    }
    Ok(Some(targets.into_iter().collect()))
}

/// `HttpError` and `HTTPError` both store as `HTTP_ERROR`, so one would read back as the other.
fn check_names(states: &[State]) -> syn::Result<()> {
    for (index, state) in states.iter().enumerate() {
        if let Some(earlier) = states[..index]
            .iter()
            .find(|earlier| earlier.name == state.name)
        {
            return Err(syn::Error::new_spanned(
                state.ident,
                format!(
                    "`{}` and `{}` both store as `{}`; rename one",
                    earlier.ident, state.ident, state.name
                ),
            ));
        }
    }
    Ok(())
}

fn check_targets(states: &[State]) -> syn::Result<()> {
    let targets = states.iter().flat_map(|state| state.next.iter().flatten());
    for target in targets {
        if !states.iter().any(|state| state.ident == target) {
            return Err(syn::Error::new_spanned(
                target,
                format!("`{target}` is not a variant of this enum"),
            ));
        }
    }
    Ok(())
}

fn variants_impl(ident: &Ident, states: &[State]) -> TokenStream {
    let type_name = ident.to_string();
    let path = enum_path(ident);
    let variants = states.iter().map(|state| state.ident);
    let names = states.iter().map(|state| &state.name);
    let edges = states.iter().flat_map(|state| {
        let from = state.ident;
        state
            .next
            .iter()
            .flatten()
            .map(move |to| quote! { (Self::#from, Self::#to) })
    });
    let spanned_ident = quote_spanned!(ident.span() => #ident);
    quote! {
        impl ::sofie::core::variants::Variants for #spanned_ident {
            const TYPE: &'static str = #type_name;
            const PATH: &'static str = #path;
            const VARIANTS: &'static [(&'static str, Self)] = &[#((#names, Self::#variants)),*];
            const TRANSITIONS: &'static [(Self, Self)] = &[#(#edges),*];
        }
    }
}

fn conversion_impls(ident: &Ident, states: &[State]) -> TokenStream {
    let names = states.iter().map(|state| &state.name);
    let serde = serde_impls(ident, &quote!(::sofie::core::variants), &quote!(*self));
    let moves = moves(ident, states);
    quote! {
        impl ::sofie::core::Cast for #ident {
            const COLUMNS: &'static [::sofie::core::ColumnSpec] = &[
                ::sofie::core::variants::COLUMNS[0].with_values(&[#(#names),*]),
            ];
            #moves

            fn encode(&self, out: &mut ::std::vec::Vec<::sofie::core::SqlValue>) {
                ::sofie::core::variants::encode(*self, out);
            }

            fn decode(row: &mut dyn ::sofie::core::ColumnReader) -> ::sofie::core::Result<Self> {
                ::sofie::core::variants::decode(row)
            }
        }

        #serde

        impl ::sofie::core::Show for #ident {
            fn show(&self) -> ::sofie::core::Shown<'_> { ::sofie::core::Shown::Text(::sofie::core::variants::Variants::name(*self)) }
        }
    }
}

/// A static list per variant, so reading where a value may move allocates nothing.
/// A literal, so a `Targets` naming it is promoted to a `'static` borrow.
fn enum_path(ident: &Ident) -> TokenStream {
    let type_name = ident.to_string();
    quote!(::core::concat!(::core::module_path!(), "::", #type_name))
}

fn moves(ident: &Ident, states: &[State]) -> TokenStream {
    if states.iter().all(|state| state.next.is_none()) {
        return TokenStream::new();
    }
    let path = enum_path(ident);
    let arms = states.iter().map(|state| {
        let variant = state.ident;
        let targets = state
            .next
            .iter()
            .flatten()
            .map(|target| screaming_snake_case(&target.unraw().to_string()));
        quote!(Self::#variant => &::sofie::core::moves::Targets { of: #path, states: &[#(#targets),*] })
    });
    quote! {
        const MOVES: ::core::option::Option<::sofie::core::moves::Moves<Self>> =
            ::core::option::Option::Some(|value| ::core::option::Option::Some(match value { #(#arms,)* }));
    }
}

fn present_impl(ident: &Ident, states: &[State]) -> TokenStream {
    let names = states.iter().map(|state| &state.name);
    quote! {
        impl ::sofie::values::Present for #ident {
            const INPUT: ::sofie::values::Input = ::sofie::values::Input::select(&[#(#names),*]);
            const SEARCH: ::sofie::values::Search = ::sofie::values::Search::Exact;

            fn csv(&self) -> ::std::string::String {
                ::sofie::core::variants::Variants::name(*self).to_owned()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_variants_storing_one_name_are_refused() {
        let input: DeriveInput = syn::parse_quote! {
            enum Failure { HttpError, HTTPError }
        };
        let error = derive(&input).expect_err("both variants store as `HTTP_ERROR`");
        assert_eq!(
            error.to_string(),
            "`HttpError` and `HTTPError` both store as `HTTP_ERROR`; rename one"
        );
    }
}
