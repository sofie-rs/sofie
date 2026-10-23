//! Each module's `src/mails/`: compiled by the app's build script, embedded
//! by the wiring and registered on the app's and the worker's database.

use proc_macro2::TokenStream;
use quote::quote;
use sofie_model::{App, MailTemplate, Module};

use super::compiled_file;

pub(crate) fn embedded_mails<'m>(
    modules: impl IntoIterator<Item = &'m Module>,
) -> impl Iterator<Item = &'m MailTemplate> {
    modules.into_iter().flat_map(|module| &module.mails)
}

/// `None` for an app without mails, whose wiring then names no mail type.
pub(crate) fn mails_table(app: &App, modules: &[&Module]) -> Option<TokenStream> {
    let entries: Vec<TokenStream> = embedded_mails(modules.iter().copied())
        .map(|mail| {
            let key = &mail.key;
            let file = compiled_file(app, &mail.file, &format!("mails/{key}.sft"));
            quote!(sofie::mail::Template::new(#key, #file))
        })
        .collect();
    if entries.is_empty() {
        return None;
    }
    Some(quote! {
        const MAILS: &[sofie::mail::Template] = &[#(#entries),*];
    })
}

pub(crate) fn register(table: Option<&TokenStream>) -> Option<TokenStream> {
    table.map(|_| quote!(sofie::mail::templates(&db, MAILS);))
}
