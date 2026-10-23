//! Arguments travel as a JSON array, answers as JSON. A trait with a method that cannot cross
//! apps gets no client or server, and `sofie split` reports it.

use proc_macro2::{TokenStream, TokenTree};
use quote::{format_ident, quote};
use syn::{
    FnArg, GenericParam, Ident, ItemTrait, ReceiverKind, ReturnType, Signature, TraitItem, Type,
    TypeReference,
};

use crate::contracts::boxed_signature;
use crate::shape::wrapped_type;

struct Method {
    signature: Signature,
    name: String,
    arguments: Vec<Argument>,
    answer: Type,
}

struct Argument {
    name: Ident,
    /// What crosses the wire: the referent for a borrowed argument.
    sent: Type,
    /// What the server reads back and lends to the implementation.
    owned: Type,
    borrowed: bool,
}

pub(crate) fn items(contract: &ItemTrait) -> TokenStream {
    let methods: Option<Vec<Method>> = contract
        .items
        .iter()
        .filter_map(|item| match item {
            TraitItem::Fn(function) => Some(method(&function.sig)),
            _ => None,
        })
        .collect();
    let Some(methods) = methods else {
        return TokenStream::new();
    };
    let ident = &contract.ident;
    let name = quote!(::core::concat!(
        ::core::env!("CARGO_PKG_NAME"),
        "/",
        ::core::stringify!(#ident)
    ));
    let client = client(ident, &name, &methods);
    let server = server(ident, &name, &methods);
    quote!(#client #server)
}

fn client(ident: &Ident, name: &TokenStream, methods: &[Method]) -> TokenStream {
    let sent = methods
        .iter()
        .flat_map(|method| &method.arguments)
        .map(|argument| &argument.sent);
    let answers = methods.iter().map(|method| &method.answer);
    let functions = methods.iter().map(|method| {
        let signature = &method.signature;
        let called = &method.name;
        let names = method.arguments.iter().map(|argument| &argument.name);
        quote! {
            #signature {
                self.call(#name, #called, ::sofie::remote::encoded(&(#(#names,)*)))
            }
        }
    });
    quote! {
        impl #ident for ::sofie::remote::Remote<dyn #ident>
        where
            #(for<'sofie_remote> #sent: ::sofie::serde::Serialize + ::core::marker::Sync,)*
            #(for<'sofie_remote> #answers: ::sofie::serde::de::DeserializeOwned + ::core::marker::Send,)*
        {
            #(#functions)*
        }
    }
}

fn server(ident: &Ident, name: &TokenStream, methods: &[Method]) -> TokenStream {
    let owned = methods
        .iter()
        .flat_map(|method| &method.arguments)
        .map(|argument| &argument.owned);
    let answers = methods.iter().map(|method| &method.answer);
    let arms = methods.iter().map(|method| {
        let function = &method.signature.ident;
        let called = &method.name;
        let names: Vec<&Ident> = method
            .arguments
            .iter()
            .map(|argument| &argument.name)
            .collect();
        let types = method.arguments.iter().map(|argument| &argument.owned);
        let passed = method.arguments.iter().map(|argument| {
            let name = &argument.name;
            if argument.borrowed {
                quote!(&#name)
            } else {
                quote!(#name)
            }
        });
        quote! {
            #called => {
                let (#(#names,)*): (#(#types,)*) = ::sofie::remote::arguments(arguments)?;
                ::sofie::remote::answered(&self.#function(#(#passed),*).await?)
            }
        }
    });
    quote! {
        impl ::sofie::remote::Serve for dyn #ident
        where
            for<'sofie_remote> dyn #ident: ::core::marker::Sync,
            #(for<'sofie_remote> #owned: ::sofie::serde::de::DeserializeOwned + ::core::marker::Send,)*
            #(for<'sofie_remote> #answers: ::sofie::serde::Serialize,)*
        {
            const CONTRACT: &'static str = #name;

            fn serve<'sofie_serve>(
                &'sofie_serve self,
                method: &'sofie_serve str,
                arguments: ::sofie::serde_json::Value,
            ) -> ::sofie::db::BoxFuture<'sofie_serve, ::sofie::core::Result<::sofie::serde_json::Value>> {
                ::std::boxed::Box::pin(async move {
                    match method {
                        #(#arms)*
                        _ => ::core::result::Result::Err(::sofie::remote::unknown_method(#name, method)),
                    }
                })
            }
        }
    }
}

/// `None` when the method cannot cross apps: not async, generic, taking `&mut`
/// or a type that borrows, or answering other than `Result`.
fn method(original: &Signature) -> Option<Method> {
    let generic = original
        .generics
        .params
        .iter()
        .any(|parameter| !matches!(parameter, GenericParam::Lifetime(_)));
    if original.asyncness.is_none() || generic {
        return None;
    }
    let mut signature = original.clone();
    let mut arguments = Vec::new();
    for (index, input) in signature.inputs.iter_mut().enumerate() {
        match input {
            FnArg::Receiver(receiver) => {
                if !matches!(receiver.kind, ReceiverKind::Reference(_, _, None)) {
                    return None;
                }
            }
            FnArg::Typed(typed) => {
                let name = format_ident!("argument_{index}");
                arguments.push(argument(name.clone(), &typed.ty)?);
                *typed.pat = syn::parse_quote!(#name);
            }
        }
    }
    let answer = match &original.output {
        ReturnType::Type(_, output) => wrapped_type(output, "Result")?.clone(),
        ReturnType::Default => return None,
    };
    if borrows(&answer) {
        return None;
    }
    boxed_signature(&mut signature);
    Some(Method {
        name: original.ident.to_string(),
        signature,
        arguments,
        answer,
    })
}

fn argument(name: Ident, ty: &Type) -> Option<Argument> {
    let Type::Reference(TypeReference {
        mutability, elem, ..
    }) = ty
    else {
        return (!borrows(ty)).then(|| Argument {
            name,
            sent: ty.clone(),
            owned: ty.clone(),
            borrowed: false,
        });
    };
    if mutability.is_some() || borrows(elem) {
        return None;
    }
    let owned = match elem.as_ref() {
        Type::Path(path) if path.path.is_ident("str") => syn::parse_quote!(::std::string::String),
        Type::Slice(slice) => {
            let item = &slice.elem;
            syn::parse_quote!(::std::vec::Vec<#item>)
        }
        other => other.clone(),
    };
    Some(Argument {
        name,
        sent: (**elem).clone(),
        owned,
        borrowed: true,
    })
}

/// A borrow or `Self` names something only the caller's side has.
fn borrows(ty: &Type) -> bool {
    names_caller_side(quote!(#ty))
}

fn names_caller_side(tokens: TokenStream) -> bool {
    tokens.into_iter().any(|token| match token {
        TokenTree::Group(group) => names_caller_side(group.stream()),
        TokenTree::Punct(punct) => matches!(punct.as_char(), '\'' | '&'),
        TokenTree::Ident(ident) => ident == "Self",
        TokenTree::Literal(_) => false,
    })
}

#[cfg(test)]
mod tests {
    use super::borrows;

    #[test]
    fn only_a_borrow_or_self_itself_stays_on_the_caller_s_side() {
        assert!(!borrows(&syn::parse_quote!(SelfService)));
        assert!(!borrows(&syn::parse_quote!(Vec<SelfService>)));
        assert!(borrows(&syn::parse_quote!(Option<&str>)));
        assert!(borrows(&syn::parse_quote!(Cow<'static, str>)));
        assert!(borrows(&syn::parse_quote!(Vec<Self>)));
    }
}
