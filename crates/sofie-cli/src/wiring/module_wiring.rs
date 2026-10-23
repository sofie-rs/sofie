use proc_macro2::TokenStream;
use quote::quote;
use sofie_model::{App, Module};

use super::{HEADER, area_mounts, job_entries, migrations_entry, route_names};

/// A module's `src/wiring.rs`, spread into its `MODULE`: written inside the crate, it names
/// routes, listeners and jobs that need not be `pub`.
pub(crate) fn render_module_wiring(app: &App, module: &Module) -> String {
    let wired = wired_module(app, module);
    let import = if module.areas.iter().any(|area| area.menu.is_some()) {
        quote!(
            use sofie::http::{Module, Navigation};
        )
    } else {
        quote!(
            use sofie::http::Module;
        )
    };
    let constant = quote!(pub(crate) const GENERATED: Module = #wired;);
    format!("{HEADER}\n{import}\n\n{constant}\n")
}

fn wired_module(app: &App, module: &Module) -> TokenStream {
    let areas = area_mounts(app, module);
    let names = route_names(app, module);
    let jobs = job_entries(module, false);
    let fields: Vec<TokenStream> = [
        migrations_entry(module).map(|migrations| quote!(migrations: Some(#migrations))),
        (!names.is_empty()).then(|| quote!(route_names: &[#(#names),*])),
        (!areas.is_empty()).then(|| quote!(areas: &[#(#areas),*])),
        (!jobs.is_empty()).then(|| quote!(jobs: |worker| worker #(#jobs)*)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let name = &module.krate.crate_name;
    if fields.is_empty() {
        return quote!(Module::new(#name));
    }
    quote!(Module { #(#fields,)* ..Module::new(#name) })
}
