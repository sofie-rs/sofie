use proc_macro2::{Ident, TokenStream};
use quote::{quote, quote_spanned};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{DeriveInput, Meta};

use crate::aggregate::{child_type, column_constants, field_description, unique_columns_check};
use crate::case::snake_case;
use crate::shape::{NamedField, named_fields};

pub(crate) fn derive(input: &DeriveInput) -> syn::Result<TokenStream> {
    let fields = named_fields(input, "Child")?;
    let (id, fields): (Vec<NamedField>, Vec<NamedField>) =
        fields.into_iter().partition(|field| field.ident == "id");
    let ident = &input.ident;
    if id.is_empty() {
        return Err(syn::Error::new_spanned(
            ident,
            "a child row needs an `id: Id<Self>` field",
        ));
    }
    let name = snake_case(&ident.to_string());
    let descriptions = fields.iter().map(field_description);
    let members: Vec<_> = fields.iter().map(|field| field.ident).collect();
    let types: Vec<_> = fields.iter().map(|field| field.ty).collect();
    let names = fields.iter().map(NamedField::name);
    let columns = column_constants(ident, &fields)?;
    let unique_columns = unique_columns_check(&fields);
    Ok(quote! {
        impl ::sofie::db::Child for #ident {
            const TABLE: ::sofie::db::ChildTable = ::sofie::db::ChildTable {
                module: ::core::env!("CARGO_CRATE_NAME"),
                name: #name,
                fields: &[#(#descriptions),*],
            };

            fn id(&self) -> ::sofie::db::Uuid {
                let id: ::sofie::db::Id<Self> = self.id;
                id.uuid()
            }

            fn encode(&self, out: &mut ::std::vec::Vec<::sofie::core::SqlValue>) {
                #(<#types as ::sofie::core::Cast>::encode(&self.#members, out);)*
            }

            fn decode(
                id: ::sofie::db::Uuid,
                row: &mut dyn ::sofie::core::ColumnReader,
            ) -> ::sofie::core::Result<Self> {
                ::core::result::Result::Ok(Self {
                    id: ::sofie::db::Id::from_uuid(id),
                    #(#members: <#types as ::sofie::core::Cast>::decode(row)?,)*
                })
            }

            fn refuse_unlisted_moves(
                &self,
                loaded: &mut dyn ::sofie::core::ColumnReader,
            ) -> ::sofie::core::Result<()> {
                #(::sofie::db::refuse_unlisted_move::<#types>(#names, &self.#members, loaded)?;)*
                ::core::result::Result::Ok(())
            }
        }

        #columns
        #unique_columns
    })
}

pub(crate) fn children_items(children: &[NamedField]) -> TokenStream {
    if children.is_empty() {
        return TokenStream::new();
    }
    let types = children.iter().filter_map(|field| child_type(field.ty));
    let members: Vec<&Ident> = children.iter().map(|field| field.ident).collect();
    quote! {
        const CHILDREN: &'static [&'static ::sofie::db::ChildTable] =
            &[#(&<#types as ::sofie::db::Child>::TABLE),*];

        fn children(&self) -> ::std::vec::Vec<&dyn ::sofie::db::ChildRows> {
            ::std::vec![#(&self.#members as &dyn ::sofie::db::ChildRows),*]
        }

        fn children_mut(&mut self) -> ::std::vec::Vec<&mut dyn ::sofie::db::ChildRows> {
            ::std::vec![#(&mut self.#members as &mut dyn ::sofie::db::ChildRows),*]
        }
    }
}

/// A child table names its parent's id column after the parent, which a child field cannot take.
pub(crate) fn parent_column_checks(parent: &str, children: &[NamedField]) -> TokenStream {
    let checks = children.iter().filter_map(|field| {
        let ty = child_type(field.ty)?;
        let message = format!(
            "a field of this child stores the `{parent}` column, which holds its parent's id; rename it"
        );
        Some(quote_spanned! {field.ty.span()=>
            const _: () = ::core::assert!(
                !::sofie::db::Field::stores(<#ty as ::sofie::db::Child>::TABLE.fields, #parent),
                #message
            );
        })
    });
    quote!(#(#checks)*)
}

pub(crate) fn parent_field(meta: &Meta) -> syn::Result<Ident> {
    let mut parent = None;
    meta.require_list()?.parse_nested_meta(|argument| {
        if !argument.path.is_ident("through") {
            return Err(argument.error("expected `through = parent_field`"));
        }
        parent = Some(argument.value()?.parse::<Ident>()?);
        Ok(())
    })?;
    parent.ok_or_else(|| syn::Error::new_spanned(meta, "expected `through = parent_field`"))
}

pub(crate) fn through_items(parent: &Ident) -> TokenStream {
    let name = parent.unraw().to_string();
    let constant = crate::shape::constant_of(parent);
    quote! {
        const OWNER: ::core::option::Option<&'static str> = ::core::option::Option::Some(#name);

        fn owned_by(owner: ::sofie::db::Uuid) -> ::core::option::Option<::sofie::db::Condition<Self>> {
            ::sofie::db::owned_through(Self::#constant, owner)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_raw_parent_field_names_its_column_without_the_prefix() {
        let parent: Ident = syn::parse_quote!(r#type);
        let items = through_items(&parent).to_string();
        assert!(items.contains("Some (\"type\")"), "{items}");
    }

    #[test]
    fn a_child_field_storing_the_parent_column_is_refused() {
        let input: DeriveInput = syn::parse_quote! {
            struct Basket { meta: Meta, items: Children<BasketItem> }
        };
        let fields = crate::shape::named_fields(&input, "Aggregate").unwrap();
        let checks = parent_column_checks("basket", &fields).to_string();
        assert!(
            checks.contains("Field :: stores (< BasketItem as :: sofie :: db :: Child > :: TABLE . fields , \"basket\")"),
            "{checks}"
        );
    }

    #[test]
    fn a_child_field_storing_an_earlier_field_s_column_is_refused() {
        let input: DeriveInput = syn::parse_quote! {
            struct OrderLine { id: Id<Self>, total: Money, total_currency: Currency }
        };
        let expanded = derive(&input).unwrap().to_string();
        assert!(
            expanded.contains("`total_currency` stores a column an earlier field already stores"),
            "{expanded}"
        );
    }
}
