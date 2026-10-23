use proc_macro2::{Ident, Span, TokenStream};
#[cfg(feature = "contracts")]
use quote::format_ident;
use quote::quote;
#[cfg(feature = "contracts")]
use syn::{FnArg, ItemTrait, ReturnType, Signature, TraitItem};
use syn::{ItemFn, Type};

use crate::shape::{NamedField, wrapped_type};

/// Higher-ranked bound: an input with a field that has no fixture compiles and has none.
pub(crate) fn input_fixture(
    ident: &Ident,
    fields: &[NamedField],
    constraints: &[TokenStream],
) -> TokenStream {
    let types: Vec<Type> = fields.iter().map(|field| as_sent(field.ty)).collect();
    let names = fields.iter().map(NamedField::name);
    quote! {
        impl ::sofie::values::InputFixture for #ident
        where
            #(for<'sofie> #types: ::sofie::values::Fixture + ::sofie::serde::Serialize,)*
        {
            fn fixture_json(seed: &mut ::sofie::values::Seed) -> ::sofie::serde_json::Value {
                let mut object = ::sofie::serde_json::Map::new();
                #(object.insert(
                    ::std::string::String::from(#names),
                    ::sofie::values::fixture::field_json::<#types>(seed, #constraints),
                );)*
                ::sofie::serde_json::Value::Object(object)
            }
        }
    }
}

/// An `Id<…>` is sent as its uuid: another crate's record id has no fixture.
fn as_sent(ty: &Type) -> Type {
    if let Some(inner) = wrapped_type(ty, "Option") {
        let inner = as_sent(inner);
        return syn::parse_quote!(::core::option::Option<#inner>);
    }
    if wrapped_type(ty, "Id").is_some() {
        return syn::parse_quote!(::sofie::core::Uuid);
    }
    ty.clone()
}

pub(crate) fn enum_fixture(ident: &Ident) -> TokenStream {
    quote! {
        impl ::sofie::values::Fixture for #ident {
            fn fixture(seed: &mut ::sofie::values::Seed) -> Self {
                ::sofie::values::fixture::variant(seed)
            }
        }
    }
}

#[cfg(feature = "contracts")]
pub(crate) fn contract_fake(contract: &ItemTrait) -> TokenStream {
    let fake = format_ident!("Fake{}", contract.ident);
    let ident = &contract.ident;
    let vis = &contract.vis;
    let mut answered = Vec::new();
    let methods: Vec<TokenStream> = contract
        .items
        .iter()
        .filter_map(|item| match item {
            TraitItem::Fn(function) if function.default.is_none() => Some(&function.sig),
            _ => None,
        })
        .map(|signature| fake_method(signature, &mut answered))
        .collect();
    let doc = format!("Records the calls made of `{ident}`; built by `#[contract]` for tests.");
    quote! {
        #[doc = #doc]
        #[derive(Default)]
        #vis struct #fake {
            pub calls: ::sofie::values::fixture::Calls,
        }

        impl #ident for #fake
        where
            #(for<'sofie_fake> #answered: ::sofie::values::Fixture,)*
        {
            #(#methods)*
        }
    }
}

#[cfg(feature = "contracts")]
fn fake_method(signature: &Signature, answered: &mut Vec<Type>) -> TokenStream {
    let name = signature.ident.to_string();
    let mut signature = signature.clone();
    let mut arguments = Vec::new();
    for argument in &mut signature.inputs {
        if let FnArg::Typed(typed) = argument {
            let ident = format_ident!("argument_{}", arguments.len());
            *typed.pat = syn::parse_quote!(#ident);
            arguments.push(ident);
        }
    }
    let output = match &signature.output {
        ReturnType::Default => syn::parse_quote!(()),
        ReturnType::Type(_, output) => (**output).clone(),
    };
    let (answer, boxed) = match wrapped_type(&output, "BoxFuture") {
        Some(inner) => (inner.clone(), true),
        None => (output, false),
    };
    let call = if let Some(inner) = wrapped_type(&answer, "Result") {
        answered.push(inner.clone());
        quote!(self.calls.answer::<#inner>(#name))
    } else {
        answered.push(answer.clone());
        quote!(self.calls.value::<#answer>(#name))
    };
    let body = if boxed {
        quote!(::std::boxed::Box::pin(::core::future::ready(#call)))
    } else {
        call
    };
    quote! {
        #signature {
            let _ = (#(#arguments,)*);
            #body
        }
    }
}

pub(crate) fn test_app(arguments: TokenStream, function: &ItemFn) -> syn::Result<TokenStream> {
    if !arguments.is_empty() {
        return Err(syn::Error::new_spanned(
            arguments,
            "write a bare `#[test_app]`: the app is the crate's own",
        ));
    }
    if function.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            function.sig.fn_token,
            "`#[test_app]` applies to an `async fn` taking the `TestApp`",
        ));
    }
    let app = app_crate(
        std::env::var("SOFIE_APP_CRATE").ok(),
        std::env::var("CARGO_PKG_NAME").ok(),
    )
    .ok_or_else(|| {
        syn::Error::new_spanned(&function.sig.ident, "`#[test_app]` runs under Cargo")
    })?;
    let app = Ident::new(&app, Span::call_site());
    let ident = &function.sig.ident;
    let attributes = &function.attrs;
    Ok(quote! {
        #[test]
        #(#attributes)*
        fn #ident() {
            #function
            ::sofie_testing::run_test_app(::#app::MODULES, ::#app::app, ::#app::worker, #ident);
        }
    })
}

/// The build script names the lib, which `[lib] name` may set apart from the package.
fn app_crate(lib: Option<String>, package: Option<String>) -> Option<String> {
    lib.or_else(|| Some(package?.replace('-', "_")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_app_calls_the_lib_by_the_name_the_build_script_gives() {
        let named = app_crate(Some("shop_app".to_owned()), Some("shop".to_owned()));
        assert_eq!(named.as_deref(), Some("shop_app"));
        let unnamed = app_crate(None, Some("my-shop".to_owned()));
        assert_eq!(unnamed.as_deref(), Some("my_shop"));
    }

    #[cfg(feature = "contracts")]
    #[test]
    fn a_fake_names_each_parameter_so_a_wildcard_or_mut_one_compiles() {
        let signature: Signature = syn::parse_quote!(fn rate(&self, _: u32, mut left: u32) -> u32);
        let method = fake_method(&signature, &mut Vec::new());
        let parsed: syn::ImplItemFn = syn::parse2(method.clone()).unwrap();
        let expected = quote!(fn rate(&self, argument_0: u32, argument_1: u32) -> u32);
        let signature = &parsed.sig;
        assert_eq!(
            quote!(#signature).to_string(),
            expected.to_string(),
            "{method}"
        );
    }
}
