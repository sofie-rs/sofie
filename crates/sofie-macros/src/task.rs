use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::ItemFn;

pub(crate) fn attribute(arguments: &TokenStream, item: TokenStream) -> TokenStream {
    if !arguments.is_empty() {
        let message = "write a bare `#[task]`: the task is named after its module and function";
        return syn::Error::new(Span::call_site(), message).into_compile_error();
    }
    match syn::parse2::<ItemFn>(item) {
        Ok(function) if function.sig.asyncness.is_none() => {
            let message = "a task is an `async fn`, as a job is";
            let error = syn::Error::new_spanned(function.sig.fn_token, message);
            let error = error.into_compile_error();
            quote!(#error #function)
        }
        Ok(function) => quote!(#function),
        Err(error) => error.into_compile_error(),
    }
}
