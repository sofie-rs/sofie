use proc_macro2::TokenStream;
use quote::quote;
use sofie_model::{Gateway, Module};

use super::in_module;

pub(super) fn gateways_function(modules: &[&Module]) -> Option<TokenStream> {
    let loads: Vec<TokenStream> = modules
        .iter()
        .flat_map(|module| {
            module
                .gateways
                .iter()
                .map(move |gateway| load(module, gateway))
        })
        .collect();
    if loads.is_empty() {
        return None;
    }
    Some(quote! {
        fn gateways() -> Result<()> {
            #(#loads)*
            Ok(())
        }
    })
}

pub(super) fn provided(module: &Module) -> impl Iterator<Item = TokenStream> {
    module.gateways.iter().map(|gateway| {
        let gateway = gateway_path(module, gateway);
        quote!(sofie::client::provide::<dyn #gateway>(db);)
    })
}

fn load(module: &Module, gateway: &Gateway) -> TokenStream {
    let name = &module.krate.crate_name;
    let key = &gateway.name;
    let choices = gateway
        .adapters
        .iter()
        .filter(|adapter| adapter.is_adapter)
        .map(|adapter| {
            let adapter = in_module(module, &adapter.ty);
            quote! {
                sofie::client::Choice::new(
                    <#adapter as sofie::client::Adapter>::VENDOR,
                    |keys| Ok(std::sync::Arc::new(sofie::client::adapter::<#adapter>(keys)?)),
                )
            }
        });
    let gateway = gateway_path(module, gateway);
    quote!(sofie::client::load::<dyn #gateway>(#name, #key, &[#(#choices),*])?;)
}

fn gateway_path(module: &Module, gateway: &Gateway) -> TokenStream {
    in_module(module, &gateway.path)
}
