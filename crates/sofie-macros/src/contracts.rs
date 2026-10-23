use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::visit_mut::{self, VisitMut};
use syn::{
    Block, DeriveInput, FnArg, GenericParam, Generics, Ident, ImplItem, Item, Lifetime, Path,
    ReceiverKind, ReturnType, Signature, TraitItem, Type, TypeImplTrait, TypeParamBound,
    TypeReference, WherePredicate,
};

use crate::shape::{NamedField, attributes_named};

/// Change events, contracts and mirrors carry what serializing shows, which leaves sensitive
/// fields out, so exposing one would drop it silently.
pub(crate) fn exposed_items(stored: &[NamedField]) -> syn::Result<TokenStream> {
    let exposed: Vec<&NamedField> = stored
        .iter()
        .filter(|field| field.has_attribute("expose"))
        .collect();
    if exposed.is_empty() {
        return Ok(TokenStream::new());
    }
    if let Some(field) = exposed
        .iter()
        .find(|field| field.has_attribute("sensitive"))
    {
        return Err(syn::Error::new_spanned(
            field.ident,
            format!("`{}` is sensitive and cannot be exposed", field.name()),
        ));
    }
    let checks = exposed.iter().map(|field| {
        let ty = field.ty;
        let message = format!("`{}` is of a sensitive type and cannot be exposed", field.name());
        quote_spanned! {ty.span()=> ::core::assert!(!<#ty as ::sofie::core::Cast>::SENSITIVE, #message);}
    });
    let names = exposed.iter().map(|field| field.name());
    Ok(quote! {
        fn exposed_fields(
            &self,
        ) -> ::core::option::Option<
            ::sofie::core::Result<::sofie::serde_json::Map<::std::string::String, ::sofie::serde_json::Value>>,
        > {
            const _: () = { #(#checks)* };
            ::core::option::Option::Some(::sofie::db::exposed_fields(self, &[#(#names),*]))
        }
    })
}

pub(crate) fn mirror_impl(input: &DeriveInput, stored: &[NamedField]) -> syn::Result<TokenStream> {
    let Some(attribute) = attributes_named(&input.attrs, "mirror").next() else {
        return Ok(TokenStream::new());
    };
    let source: Path = attribute.parse_args()?;
    let mut changed = source.clone();
    let Some(last) = changed.segments.last_mut() else {
        return Err(syn::Error::new_spanned(
            attribute,
            "name the aggregate mirrored",
        ));
    };
    last.ident = format_ident!("{}Changed", last.ident);
    let ident = &input.ident;
    let names = stored.iter().map(NamedField::name);
    let types = stored.iter().map(|field| field.ty);
    Ok(quote! {
        impl ::sofie::db::Mirror for #ident {
            type Source = #source;
            type Changed = #changed;

            fn columns(
                changed: &::sofie::serde_json::Map<::std::string::String, ::sofie::serde_json::Value>,
            ) -> ::sofie::core::Result<::std::vec::Vec<::sofie::core::SqlValue>> {
                let mut columns = ::std::vec::Vec::new();
                #(<#types as ::sofie::core::Cast>::encode(
                    &::sofie::db::changed_field::<#types>(changed, #names)?,
                    &mut columns,
                );)*
                ::core::result::Result::Ok(columns)
            }
        }
    })
}

/// Contracts and gateways box methods alike; only the span a call runs in differs.
#[derive(Clone, Copy)]
pub(crate) enum Boundary {
    Contract,
    #[cfg(feature = "gateways")]
    Gateway,
}

pub(crate) fn attribute(item: TokenStream, boundary: Boundary) -> TokenStream {
    syn::parse2(item)
        .and_then(|item| contract(item, boundary))
        .unwrap_or_else(syn::Error::into_compile_error)
}

fn contract(item: Item, boundary: Boundary) -> syn::Result<TokenStream> {
    match item {
        Item::Trait(mut contract) => {
            #[cfg(feature = "remote-contracts")]
            let remote = match boundary {
                Boundary::Contract => crate::remote::items(&contract),
                #[cfg(feature = "gateways")]
                Boundary::Gateway => TokenStream::new(),
            };
            #[cfg(not(feature = "remote-contracts"))]
            let remote = TokenStream::new();
            for item in &mut contract.items {
                let TraitItem::Fn(function) = item else {
                    continue;
                };
                if !boxed_signature(&mut function.sig) {
                    continue;
                }
                if let Some(body) = &function.default {
                    function.default = Some(traced(&contract.ident, &function.sig.ident, body));
                }
            }
            #[cfg(feature = "fixtures")]
            let contract = {
                let fake = crate::fixture::contract_fake(&contract);
                quote!(#contract #fake)
            };
            Ok(quote!(#contract #remote))
        }
        Item::Impl(mut implementation) => {
            let Some(contract) = implementation
                .trait_
                .as_ref()
                .and_then(|(path, _)| path.segments.last())
                .map(|segment| segment.ident.clone())
            else {
                return Err(syn::Error::new_spanned(
                    implementation.self_ty,
                    "`#[contract]` and `#[gateway]` apply to an implementation of their trait",
                ));
            };
            for item in &mut implementation.items {
                let ImplItem::Fn(function) = item else {
                    continue;
                };
                if !boxed_signature(&mut function.sig) {
                    continue;
                }
                function.block = match boundary {
                    Boundary::Contract => traced(&contract, &function.sig.ident, &function.block),
                    #[cfg(feature = "gateways")]
                    Boundary::Gateway => vendor_call(&function.sig.ident, &function.block),
                };
            }
            Ok(quote!(#implementation))
        }
        other => Err(syn::Error::new_spanned(
            other,
            "`#[contract]` and `#[gateway]` apply to a trait and its implementations",
        )),
    }
}

/// Filed under the implementing module, as a handler's span is.
fn traced(contract: &Ident, function: &Ident, body: &Block) -> Block {
    let name = format!("{contract}::{function}");
    let telemetry = quote!(::sofie::telemetry::__tracing);
    syn::parse_quote!({
        ::std::boxed::Box::pin(#telemetry::Instrument::instrument(
            async move #body,
            #telemetry::info_span!(
                "contract",
                otel.name = #name,
                module = ::core::module_path!().split("::").next(),
            ),
        ))
    })
}

#[cfg(feature = "gateways")]
fn vendor_call(function: &Ident, body: &Block) -> Block {
    let operation = function.to_string();
    syn::parse_quote!({
        ::std::boxed::Box::pin(::sofie::client::call(
            <Self as ::sofie::client::Adapter>::VENDOR,
            #operation,
            async move #body,
        ))
    })
}

pub(crate) fn boxed_signature(signature: &mut Signature) -> bool {
    if signature.asyncness.take().is_none() {
        return false;
    }
    let lifetime = Lifetime::new("'sofie", Span::call_site());
    outlive(&mut signature.generics, &lifetime);
    for argument in &mut signature.inputs {
        match argument {
            FnArg::Receiver(receiver) => {
                if let ReceiverKind::Reference(_, named, _) = &mut receiver.kind {
                    named.get_or_insert_with(|| lifetime.clone());
                }
            }
            FnArg::Typed(typed) => Borrowed(&lifetime).visit_type_mut(&mut typed.ty),
        }
    }
    let output: Type = match &signature.output {
        ReturnType::Default => syn::parse_quote!(()),
        ReturnType::Type(_, output) => (**output).clone(),
    };
    signature.output = syn::parse_quote!(-> ::sofie::db::BoxFuture<#lifetime, #output>);
    true
}

/// The boxed future holds every argument, so each generic it names must outlive it.
fn outlive(generics: &mut Generics, lifetime: &Lifetime) {
    let bounds: Vec<WherePredicate> = generics
        .params
        .iter()
        .filter_map(|param| match param {
            GenericParam::Lifetime(named) => {
                let named = &named.lifetime;
                Some(syn::parse_quote!(#named: #lifetime))
            }
            GenericParam::Type(ty) => {
                let ty = &ty.ident;
                Some(syn::parse_quote!(#ty: #lifetime))
            }
            GenericParam::Const(_) => None,
        })
        .collect();
    generics.params.insert(0, syn::parse_quote!(#lifetime));
    generics.make_where_clause().predicates.extend(bounds);
}

/// Names every elided borrow in an argument's type, nested ones included, after the future.
struct Borrowed<'a>(&'a Lifetime);

impl VisitMut for Borrowed<'_> {
    fn visit_type_reference_mut(&mut self, reference: &mut TypeReference) {
        reference.lifetime.get_or_insert_with(|| self.0.clone());
        visit_mut::visit_type_reference_mut(self, reference);
    }

    fn visit_lifetime_mut(&mut self, lifetime: &mut Lifetime) {
        if lifetime.ident == "_" {
            *lifetime = self.0.clone();
        }
    }

    fn visit_type_impl_trait_mut(&mut self, impl_trait: &mut TypeImplTrait) {
        impl_trait
            .bounds
            .push(TypeParamBound::Lifetime(self.0.clone()));
        visit_mut::visit_type_impl_trait_mut(self, impl_trait);
    }
}

#[cfg(test)]
mod tests {
    use syn::{DeriveInput, Signature};

    use super::{boxed_signature, exposed_items};
    use crate::shape::named_fields;

    #[test]
    fn a_sensitive_field_cannot_be_exposed() {
        let input: DeriveInput = syn::parse_quote! {
            struct Account { #[expose] name: Text, #[expose] #[sensitive] recovery_code: Text }
        };
        let fields = named_fields(&input, "Aggregate").unwrap();
        let error = exposed_items(&fields).expect_err("a sensitive field is refused");
        assert!(
            error.to_string().contains("`recovery_code` is sensitive"),
            "{error}"
        );
    }

    #[test]
    fn an_exposed_field_must_not_be_of_a_sensitive_type() {
        let input: DeriveInput = syn::parse_quote! {
            struct Payee { #[expose] iban: Iban, name: Text }
        };
        let fields = named_fields(&input, "Aggregate").unwrap();
        let exposed = exposed_items(&fields).unwrap().to_string();
        assert!(
            exposed.contains("assert ! (! < Iban as :: sofie :: core :: Cast > :: SENSITIVE"),
            "{exposed}"
        );
        assert!(!exposed.contains("< Text as"), "{exposed}");
    }

    #[test]
    fn every_borrow_an_argument_holds_lives_as_long_as_the_boxed_future() {
        let mut signature: Signature = syn::parse_quote! {
            async fn find<'a, T>(&self, name: Option<&str>, tags: &[&str], each: impl Fn(), kept: &'a T, of: Ref<'_, u8>) -> u8
        };
        boxed_signature(&mut signature);
        let boxed = quote::quote!(#signature).to_string();
        for expected in [
            "name : Option < & 'sofie str >",
            "tags : & 'sofie [& 'sofie str]",
            "each : impl Fn () + 'sofie",
            "of : Ref < 'sofie , u8 >",
            "'a : 'sofie",
            "T : 'sofie",
        ] {
            assert!(boxed.contains(expected), "{expected} in {boxed}");
        }
    }
}
