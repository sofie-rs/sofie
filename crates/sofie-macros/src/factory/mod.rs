use std::path::{Path, PathBuf};

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use syn::DeriveInput;

#[cfg(feature = "serialize")]
mod record;

use crate::aggregate::Owner;
use crate::input::{Options, dto_impls};
use crate::shape::NamedField;

/// Not generated when the module writes its own `factory.rs`: it has rules the defaults would skip.
pub(crate) fn default_factory(
    input: &DeriveInput,
    meta: &NamedField,
    stored: &[NamedField],
    children: &[NamedField],
    owner: Option<&Owner>,
) -> syn::Result<TokenStream> {
    // Given by the area, not the input.
    let owner = match owner {
        Some(Owner::Field(field)) => Some(*field),
        _ => None,
    };
    let defaults = defaults(input, meta, stored, children, owner)?;
    #[cfg(feature = "serialize")]
    let defaults = {
        let serialize = record::serialize_impl(&input.ident, stored);
        quote!(#defaults #serialize)
    };
    Ok(defaults)
}

fn defaults(
    input: &DeriveInput,
    meta: &NamedField,
    stored: &[NamedField],
    children: &[NamedField],
    owner: Option<&NamedField>,
) -> syn::Result<TokenStream> {
    if writes_factory() {
        return Ok(TokenStream::new());
    }
    let ident = &input.ident;
    let vis = &input.vis;
    let create = format_ident!("Create{ident}");
    let update = format_ident!("Update{ident}");
    let owner_ident = owner.map(|owner| owner.ident);
    let fields: Vec<NamedField> = stored
        .iter()
        .filter(|field| Some(field.ident) != owner_ident)
        .copied()
        .collect();
    let members: Vec<&Ident> = fields.iter().map(|field| field.ident).collect();
    let types = fields.iter().map(|field| field.ty);
    let dto = dto_impls(&create, &fields, &Options::default())?;
    let (owner_type, owner_pattern, owner_init) = match owner {
        Some(owner) => {
            let (member, ty) = (owner.ident, owner.ty);
            (
                quote!(#ty),
                quote!(sofie_owner),
                quote!(#member: sofie_owner,),
            )
        }
        None => (quote!(()), quote!(()), TokenStream::new()),
    };
    let meta = meta.ident;
    let children = children.iter().map(|field| field.ident);
    Ok(quote! {
        #vis struct #create {
            #(#vis #members: #types,)*
        }

        #vis type #update = #create;

        #dto

        impl ::sofie::db::Create for #ident {
            type Input = #create;
            type Owner = #owner_type;

            fn create(
                #create { #(#members),* }: #create,
                #owner_pattern: #owner_type,
            ) -> ::sofie::core::Result<Self> {
                ::core::result::Result::Ok(Self {
                    #meta: ::sofie::db::Meta::new()?,
                    #owner_init
                    #(#members,)*
                    #(#children: ::core::default::Default::default(),)*
                })
            }
        }

        impl ::sofie::db::Update for #ident {
            type Input = #create;

            fn update(&mut self, #create { #(#members),* }: #create) -> ::sofie::core::Result<()> {
                #(self.#members = #members;)*
                ::core::result::Result::Ok(())
            }
        }
    })
}

fn writes_factory() -> bool {
    // A derive run by a unit test has no call site to read.
    if !proc_macro::is_available() {
        return false;
    }
    let Some(file) = proc_macro::Span::call_site().local_file() else {
        return false;
    };
    let folder = module_folder(&file);
    folder.join("factory.rs").is_file() || folder.join("factory").join("mod.rs").is_file()
}

fn module_folder(file: &Path) -> PathBuf {
    let is_root = file
        .file_stem()
        .is_some_and(|stem| stem == "mod" || stem == "lib" || stem == "main");
    match file.parent() {
        Some(parent) if is_root => parent.to_path_buf(),
        _ => file.with_extension(""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_s_children_live_in_its_folder() {
        assert_eq!(
            module_folder(Path::new("src/order/mod.rs")),
            Path::new("src/order")
        );
        assert_eq!(
            module_folder(Path::new("src/product.rs")),
            Path::new("src/product")
        );
        assert_eq!(module_folder(Path::new("src/lib.rs")), Path::new("src"));
    }
}
