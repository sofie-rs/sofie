use proc_macro2::{Ident, TokenStream};
use quote::quote;
use syn::ext::IdentExt;
use syn::{Attribute, Data, DeriveInput, Fields, GenericArgument, PathArguments, Type};

#[derive(Clone, Copy)]
pub(crate) struct NamedField<'a> {
    pub(crate) ident: &'a Ident,
    pub(crate) ty: &'a Type,
    pub(crate) attrs: &'a [Attribute],
}

impl NamedField<'_> {
    /// The field's name as stored and serialized, without any `r#`.
    pub(crate) fn name(&self) -> String {
        self.ident.unraw().to_string()
    }

    pub(crate) fn constant(&self) -> Ident {
        constant_of(self.ident)
    }

    pub(crate) fn has_attribute(&self, name: &str) -> bool {
        attributes_named(self.attrs, name).next().is_some()
    }
}

pub(crate) fn names_with<'a>(
    fields: &'a [NamedField],
    attribute: &'a str,
) -> impl Iterator<Item = String> + 'a {
    fields
        .iter()
        .filter(move |field| field.has_attribute(attribute))
        .map(NamedField::name)
}

pub(crate) fn constant_of(field: &Ident) -> Ident {
    Ident::new(&field.unraw().to_string().to_uppercase(), field.span())
}

pub(crate) fn reject_generics(input: &DeriveInput, derive: &str) -> syn::Result<()> {
    if input.generics.params.is_empty() && input.generics.where_clause.is_none() {
        return Ok(());
    }
    Err(syn::Error::new_spanned(
        &input.generics,
        format!("`#[derive({derive})]` does not support generics"),
    ))
}

pub(crate) fn struct_fields<'a>(input: &'a DeriveInput, derive: &str) -> syn::Result<&'a Fields> {
    match &input.data {
        Data::Struct(data) => Ok(&data.fields),
        _ => Err(syn::Error::new_spanned(
            &input.ident,
            format!("`#[derive({derive})]` applies to structs only"),
        )),
    }
}

pub(crate) fn named_fields<'a>(
    input: &'a DeriveInput,
    derive: &str,
) -> syn::Result<Vec<NamedField<'a>>> {
    reject_generics(input, derive)?;
    let Fields::Named(fields) = struct_fields(input, derive)? else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            format!("`#[derive({derive})]` needs a struct with named fields"),
        ));
    };
    fields
        .named
        .iter()
        .map(|field| {
            let ident = field
                .ident
                .as_ref()
                .ok_or_else(|| syn::Error::new_spanned(field, "expected a named field"))?;
            Ok(NamedField {
                ident,
                ty: &field.ty,
                attrs: &field.attrs,
            })
        })
        .collect()
}

pub(crate) fn resolve_impl(
    ident: &Ident,
    bound: &TokenStream,
    future: &TokenStream,
) -> TokenStream {
    quote! {
        impl<SofieContext: #bound> ::sofie::db::Resolve<SofieContext> for #ident {
            fn resolve(
                context: &mut SofieContext,
            ) -> impl ::core::future::Future<Output = ::sofie::core::Result<Self>> + ::core::marker::Send {
                #future
            }
        }
    }
}

pub(crate) fn attributes_named<'a>(
    attributes: &'a [Attribute],
    name: &'a str,
) -> impl Iterator<Item = &'a Attribute> {
    attributes
        .iter()
        .filter(move |attribute| attribute.path().is_ident(name))
}

/// The first type argument of `wrapper<…>`, after any lifetime.
pub(crate) fn wrapped_type<'a>(ty: &'a Type, wrapper: &str) -> Option<&'a Type> {
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if path.qself.is_some() || segment.ident != wrapper {
        return None;
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    arguments.args.iter().find_map(|argument| match argument {
        GenericArgument::Type(inner) => Some(inner),
        _ => None,
    })
}
