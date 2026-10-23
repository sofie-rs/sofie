use proc_macro2::TokenStream;
use quote::quote;
use sofie_model::naming::job_name;
use sofie_model::{Module, Provision, WiringSide};

use super::gates::job_function;
use super::{exposed_aggregates, in_module, path, rooted};

pub(super) fn provide_function(
    modules: &[&Module],
    remote: Vec<TokenStream>,
) -> Option<TokenStream> {
    let mut statements: Vec<TokenStream> = modules
        .iter()
        .flat_map(|module| provisions(module))
        .collect();
    statements.extend(remote);
    if statements.is_empty() {
        return None;
    }
    Some(quote! {
        fn provide(db: &Db) {
            #(#statements)*
        }
    })
}

fn provisions(module: &Module) -> Vec<TokenStream> {
    let mut statements: Vec<TokenStream> = module
        .implementations
        .iter()
        .map(|provision| provided(module, provision, &quote!(provide)))
        .collect();
    statements.extend(lookups(module));
    statements.extend(super::gateways::provided(module));
    statements.extend(
        module
            .extensions
            .iter()
            .map(|provision| provided(module, provision, &quote!(extend))),
    );
    statements.extend(module.jobs.iter().map(|job| {
        let job_name = job_name(module, &job.function);
        let function = in_module(module, &job.function);
        quote!(sofie::db::name_job(db, #job_name, #function);)
    }));
    statements
}

fn provided(module: &Module, provision: &Provision, method: &TokenStream) -> TokenStream {
    let contract = path(&provision.contract.join("::"));
    let ty = in_module(module, &provision.ty);
    let built = if provision.unit {
        ty
    } else {
        quote!(#ty::from(db.clone()))
    };
    quote!(sofie::db::#method::<dyn #contract>(db, std::sync::Arc::new(#built));)
}

fn lookups(module: &Module) -> Vec<TokenStream> {
    let Some(contract) = &module.contract else {
        return Vec::new();
    };
    exposed_aggregates(module)
        .map(|aggregate| {
            let dto = path(&format!("{}::{}", contract.crate_name, aggregate.name));
            let aggregate = in_module(module, &aggregate.path);
            quote! {
                sofie::db::provide::<dyn sofie::db::Lookup<#dto>>(
                    db,
                    std::sync::Arc::new(sofie::db::Exposed::<#aggregate, #dto>::new(db.clone())),
                );
            }
        })
        .collect()
}

/// A mirror's listener is never gated.
pub(super) fn worker_entries(module: &Module, gated: bool) -> Vec<TokenStream> {
    let mut entries: Vec<TokenStream> = module
        .jobs
        .iter()
        .filter(|job| module.code.features.gate_on(&job.function).is_some() == gated)
        .map(|job| {
            let name = job_name(module, &job.function);
            let function = job_function(module, &job.function);
            quote!(.job(#name, #function))
        })
        .collect();
    if gated {
        return entries;
    }
    entries.extend(
        module
            .code
            .aggregates
            .iter()
            .filter(|aggregate| aggregate.mirror.is_some())
            .map(|mirror| {
                let name = job_name(module, &mirror.path);
                let mirror = rooted(module, WiringSide::Module, &mirror.path);
                quote!(.mirror::<#mirror>(#name))
            }),
    );
    entries
}
