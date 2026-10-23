use proc_macro2::{Group, Spacing, TokenStream, TokenTree};
use quote::ToTokens;
use syn::{DeriveInput, Ident, Path};

use crate::shape::attributes_named;

/// Crates below the facade cannot name `::sofie`, so `#[sofie(crate = path)]` puts their own
/// path wherever a derive wrote `::sofie`; derives keep writing `::sofie` and stay unaware.
pub(crate) fn with_crate_path(
    input: &DeriveInput,
    expanded: TokenStream,
) -> syn::Result<TokenStream> {
    Ok(match crate_path(input)? {
        Some(path) => replace_facade(expanded, &path.to_token_stream()),
        None => expanded,
    })
}

fn crate_path(input: &DeriveInput) -> syn::Result<Option<Path>> {
    let mut path = None;
    for attribute in attributes_named(&input.attrs, "sofie") {
        attribute.parse_nested_meta(|meta| {
            if !meta.path.is_ident("crate") {
                return Err(meta.error("expected `crate = path`"));
            }
            path = Some(meta.value()?.parse()?);
            Ok(())
        })?;
    }
    Ok(path)
}

fn replace_facade(tokens: TokenStream, facade: &TokenStream) -> TokenStream {
    let tokens: Vec<TokenTree> = tokens.into_iter().collect();
    let mut replaced = TokenStream::new();
    let mut index = 0;
    while index < tokens.len() {
        if names_facade(&tokens[index..]) && !continues_path(&tokens[..index]) {
            replaced.extend(facade.clone());
            index += 3;
            continue;
        }
        replaced.extend([match &tokens[index] {
            TokenTree::Group(group) => {
                let mut inner =
                    Group::new(group.delimiter(), replace_facade(group.stream(), facade));
                inner.set_span(group.span());
                TokenTree::Group(inner)
            }
            other => other.clone(),
        }]);
        index += 1;
    }
    replaced
}

fn names_facade(tokens: &[TokenTree]) -> bool {
    match tokens {
        [
            TokenTree::Punct(first),
            TokenTree::Punct(second),
            TokenTree::Ident(name),
            ..,
        ] => {
            first.as_char() == ':'
                && first.spacing() == Spacing::Joint
                && second.as_char() == ':'
                && name == "sofie"
        }
        _ => false,
    }
}

/// After a path segment (`module::sofie`, `<T as Trait>::sofie`) the `::sofie` is the user's own;
/// only a path that starts with `::sofie` names the facade.
fn continues_path(before: &[TokenTree]) -> bool {
    match before {
        [.., TokenTree::Punct(tick), TokenTree::Ident(_)] if tick.as_char() == '\'' => false,
        [.., TokenTree::Ident(segment)] => {
            matches!(
                segment.to_string().as_str(),
                "self" | "super" | "crate" | "Self"
            ) || syn::parse2::<Ident>(segment.to_token_stream()).is_ok()
        }
        [.., TokenTree::Punct(close)] if close.as_char() == '>' => closes_a_type(before),
        _ => false,
    }
}

/// `Vec<u8>` and `<T as Trait>` are types a path goes on from; `impl<T>` and `for<'a>` are not.
fn closes_a_type(before: &[TokenTree]) -> bool {
    let mut depth = 0_usize;
    for index in (0..before.len()).rev() {
        let TokenTree::Punct(punct) = &before[index] else {
            continue;
        };
        match punct.as_char() {
            '>' if !is_arrow(&before[..index]) => depth += 1,
            '>' if depth == 0 => return false,
            '<' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return !matches!(
                before[..index].last(),
                Some(TokenTree::Ident(word)) if word == "impl" || word == "for"
            );
        }
    }
    false
}

/// Whether a `>` after these tokens ends `->` or `=>`.
fn is_arrow(before: &[TokenTree]) -> bool {
    matches!(
        before.last(),
        Some(TokenTree::Punct(head))
            if head.spacing() == Spacing::Joint && matches!(head.as_char(), '-' | '=')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crate_attribute_replaces_the_facade_path() {
        let input: DeriveInput = syn::parse_quote! {
            #[sofie(crate = crate::facade)]
            enum Status { Open }
        };
        let expanded = quote::quote! { impl ::sofie::core::Cast for Status { fn f(_: <T as ::sofie::X>::Y) {} } };
        let replaced = with_crate_path(&input, expanded).expect("the attribute parses");
        assert_eq!(
            replaced.to_string(),
            quote::quote! { impl crate::facade::core::Cast for Status { fn f(_: <T as crate::facade::X>::Y) {} } }
                .to_string()
        );
    }

    #[test]
    fn a_user_path_through_a_module_named_sofie_is_kept() {
        let input: DeriveInput = syn::parse_quote! {
            #[sofie(crate = crate::facade)]
            enum Status { Open }
        };
        let expanded = quote::quote! {
            fn f(_: app::sofie::Money, _: <T as Trait>::sofie, _: crate::sofie::X) -> ::sofie::Y {
                match x { _ => ::sofie::Z }
            }
            type A<'a> = &'a ::sofie::W;
            impl<T> ::sofie::V for Vec<T> where for<'a> ::sofie::U<'a>: Sized {}
        };
        let replaced = with_crate_path(&input, expanded).expect("the attribute parses");
        assert_eq!(
            replaced.to_string(),
            quote::quote! {
                fn f(_: app::sofie::Money, _: <T as Trait>::sofie, _: crate::sofie::X) -> crate::facade::Y {
                    match x { _ => crate::facade::Z }
                }
                type A<'a> = &'a crate::facade::W;
                impl<T> crate::facade::V for Vec<T> where for<'a> crate::facade::U<'a>: Sized {}
            }
            .to_string()
        );
    }

    #[test]
    fn without_the_attribute_the_facade_stays_sofie() {
        let input: DeriveInput = syn::parse_quote! { enum Status { Open } };
        let expanded = quote::quote! { impl ::sofie::core::Cast for Status {} };
        let replaced = with_crate_path(&input, expanded.clone()).expect("nothing to parse");
        assert_eq!(replaced.to_string(), expanded.to_string());
    }

    #[test]
    fn an_unknown_key_is_refused() {
        let input: DeriveInput = syn::parse_quote! {
            #[sofie(krate = crate)]
            enum Status { Open }
        };
        let error = with_crate_path(&input, TokenStream::new()).expect_err("only `crate` is known");
        assert_eq!(error.to_string(), "expected `crate = path`");
    }
}
