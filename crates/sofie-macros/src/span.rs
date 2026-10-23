use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::ItemFn;

pub(crate) fn attribute(arguments: &TokenStream, item: TokenStream) -> TokenStream {
    if !arguments.is_empty() {
        let message = "write a bare `#[span]`: the span is named after the function";
        return syn::Error::new(Span::call_site(), message).into_compile_error();
    }
    match syn::parse2::<ItemFn>(item) {
        Ok(function) => span(function),
        Err(error) => error.into_compile_error(),
    }
}

/// An `async` body is instrumented rather than entered: it may be polled on any thread.
fn span(mut function: ItemFn) -> TokenStream {
    let name = function.sig.ident.to_string();
    let telemetry = quote!(::sofie::telemetry::__tracing);
    let opened = quote! {
        #telemetry::info_span!(
            "step",
            otel.name = concat!(module_path!(), "::", #name),
            module = module_path!(),
        )
    };
    let body = &function.block;
    let wrapped = if function.sig.asyncness.is_some() {
        quote!({ #telemetry::Instrument::instrument(async move #body, #opened).await })
    } else {
        quote!({ let _sofie_step = #opened.entered(); #body })
    };
    *function.block = syn::parse_quote!(#wrapped);
    quote!(#function)
}
