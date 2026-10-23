use proc_macro2::{Ident, TokenStream};
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{DeriveInput, GenericArgument, Path, PathArguments, Type};

#[cfg(feature = "joins")]
use crate::joins::join_of;
use crate::serialize::{REACHABLE_SUFFIX, Reachable, record_impl, serialize_impl};
use crate::shape::{NamedField, attributes_named, named_fields, resolve_impl};

pub(crate) fn derive(input: &DeriveInput) -> syn::Result<TokenStream> {
    let fields = named_fields(input, "ReadModel")?;
    reject_reserved_names(&fields)?;
    let Some(base) = base_aggregate(input)? else {
        return computed(input, &fields);
    };
    let joins = fields
        .iter()
        .map(join_of)
        .collect::<syn::Result<Vec<_>>>()?;
    let ident = &input.ident;
    let sensitive = crate::sensitive::items(ident, &fields);
    let checks = field_checks(&base, &fields, &joins);
    let shown = shown_checks(ident, &base, &fields, &joins);
    let source = source_impl(ident, &base, &fields, &joins);
    let serialize = serialize_impl(ident, &fields, Reachable::Included);
    let record = record_impl(ident, &fields, Reachable::Included);
    let outcome = outcome_impl(ident);
    let resolve = resolve_impl(
        ident,
        &quote!(::sofie::db::Context),
        &quote!(::sofie::db::bind::<Self, SofieContext>(context)),
    );
    Ok(quote! { #checks #shown #source #serialize #record #outcome #resolve #sensitive })
}

/// Without `from` the loader fills the fields itself, so there is nothing to query or bind.
fn computed(input: &DeriveInput, fields: &[NamedField]) -> syn::Result<TokenStream> {
    if let Some(attribute) = fields
        .iter()
        .find_map(|field| attributes_named(field.attrs, "read").next())
    {
        return Err(syn::Error::new_spanned(
            attribute,
            "a joined field needs the read model's `#[read(from = Aggregate)]`",
        ));
    }
    let ident = &input.ident;
    let typed = typed_checks(ident, fields);
    let serialize = serialize_impl(ident, fields, Reachable::Omitted);
    let record = record_impl(ident, fields, Reachable::Omitted);
    let outcome = outcome_impl(ident);
    let sensitive = crate::sensitive::items(ident, fields);
    Ok(quote! { #typed #serialize #record #outcome #sensitive })
}

/// A computed field may be of any type, so a type without `Cast` falls back to the trait's
/// `false`: an inherent constant applies only where its bounds hold.
fn typed_checks(ident: &Ident, fields: &[NamedField]) -> TokenStream {
    let checks = fields
        .iter()
        .filter(|field| !field.has_attribute("sensitive"))
        .flat_map(|field| {
            let message = format!(
                "`{ident}.{}` is of a sensitive type; mark it `#[sensitive]` to send it on purpose",
                field.name()
            );
            held_types(field.ty).into_iter().map(move |ty| {
                quote_spanned! {ty.span() => assert!(!<SofieSensitivity<#ty>>::SENSITIVE, #message);}
            })
        });
    quote! {
        const _: () = {
            struct SofieSensitivity<T: ?::core::marker::Sized>(::core::marker::PhantomData<T>);
            impl<T: ::sofie::core::Cast> SofieSensitivity<T> {
                const SENSITIVE: bool = <T as ::sofie::core::Cast>::SENSITIVE;
            }
            trait SofieUnsensitive {
                const SENSITIVE: bool = false;
            }
            impl<T: ?::core::marker::Sized> SofieUnsensitive for SofieSensitivity<T> {}
            #(#checks)*
        };
    }
}

/// The type and every type it holds, as `Vec<Iban>` or a map's values send each `Iban` they hold.
fn held_types(ty: &Type) -> Vec<&Type> {
    let mut held = vec![ty];
    match ty {
        Type::Path(path) => {
            let arguments =
                path.path
                    .segments
                    .iter()
                    .flat_map(|segment| match &segment.arguments {
                        PathArguments::AngleBracketed(arguments) => arguments.args.iter().collect(),
                        _ => Vec::new(),
                    });
            for argument in arguments {
                if let GenericArgument::Type(inner) = argument {
                    held.extend(held_types(inner));
                }
            }
        }
        Type::Array(array) => held.extend(held_types(&array.elem)),
        Type::Slice(slice) => held.extend(held_types(&slice.elem)),
        Type::Reference(reference) => held.extend(held_types(&reference.elem)),
        Type::Paren(paren) => held.extend(held_types(&paren.elem)),
        Type::Group(group) => held.extend(held_types(&group.elem)),
        Type::Tuple(tuple) => held.extend(tuple.elems.iter().flat_map(held_types)),
        _ => {}
    }
    held
}

fn base_aggregate(input: &DeriveInput) -> syn::Result<Option<Path>> {
    let mut base = None;
    for attribute in attributes_named(&input.attrs, "read") {
        attribute.parse_nested_meta(|meta| {
            if !meta.path.is_ident("from") {
                return Err(meta.error("expected `from = Aggregate`"));
            }
            base = Some(meta.value()?.parse::<Path>()?);
            Ok(())
        })?;
    }
    Ok(base)
}

pub(crate) struct Join {
    pub(crate) via: Path,
    pub(crate) field: Path,
}

#[cfg(not(feature = "joins"))]
fn join_of(field: &NamedField) -> syn::Result<Option<Join>> {
    match attributes_named(field.attrs, "read").next() {
        Some(attribute) => Err(syn::Error::new_spanned(
            attribute,
            "a joined field needs the framework's `joins` feature",
        )),
        None => Ok(None),
    }
}

fn reject_reserved_names(fields: &[NamedField]) -> syn::Result<()> {
    let names: Vec<String> = fields.iter().map(NamedField::name).collect();
    for (field, name) in fields.iter().zip(&names) {
        let base = name.strip_suffix(REACHABLE_SUFFIX);
        if base.is_some_and(|base| names.iter().any(|name| name == base)) {
            return Err(syn::Error::new_spanned(
                field.ident,
                format!("`{name}` is reserved for the states its field may move to; rename it"),
            ));
        }
    }
    Ok(())
}

/// The compiler checks each field's type through the typed column constants.
fn field_checks(base: &Path, fields: &[NamedField], joins: &[Option<Join>]) -> TokenStream {
    let checks = fields.iter().zip(joins).map(|(field, join)| {
        let ty = field.ty;
        if let Some(Join { via, field }) = join {
            return quote_spanned! {ty.span() =>
                let _: ::sofie::db::Column<#base, _> = #via;
                ::sofie::db::joined_field::<_, _, #ty>(#field);
            };
        }
        let column = if field.ident == "id" {
            quote! { ::sofie::db::Column::<#base, ::sofie::db::Id<#base>>::new("id") }
        } else {
            let constant = field.constant();
            quote! { #base::#constant }
        };
        quote_spanned! {ty.span() =>
            let _: ::sofie::db::Column<#base, #ty> = #column;
        }
    });
    quote! {
        const _: () = {
            fn check() {
                #(#checks)*
            }
        };
    }
}

/// Everything selected reaches JSON and tables, so a sensitive field is shown only on purpose.
fn shown_checks(
    ident: &Ident,
    base: &Path,
    fields: &[NamedField],
    joins: &[Option<Join>],
) -> TokenStream {
    let checks = fields
        .iter()
        .zip(joins)
        .filter(|(field, _)| field.ident != "id" && !field.has_attribute("sensitive"))
        .map(|(field, join)| {
            let column = if let Some(Join { field, .. }) = join {
                quote!(#field)
            } else {
                let constant = field.constant();
                quote!(#base::#constant)
            };
            let message = format!(
                "`{ident}.{}` reads a sensitive field; mark it `#[sensitive]` to send it on purpose",
                field.name()
            );
            quote_spanned! {field.ident.span() => assert!(readable(#column), #message);}
        });
    quote! {
        const _: () = {
            const fn readable<A: ::sofie::db::Aggregate, T>(column: ::sofie::db::Column<A, T>) -> bool {
                let (name, mut index) = (column.name().as_bytes(), 0);
                while index < A::SENSITIVE.len() {
                    if A::SENSITIVE[index].as_bytes().eq_ignore_ascii_case(name) {
                        return false;
                    }
                    index += 1;
                }
                true
            }
            #(#checks)*
        };
    }
}

fn source_impl(
    ident: &Ident,
    base: &Path,
    fields: &[NamedField],
    joins: &[Option<Join>],
) -> TokenStream {
    let selections = fields.iter().zip(joins).map(|(field, join)| {
        if let Some(Join { via, field }) = join {
            return quote! { ::sofie::db::joined(#via, #field) };
        }
        let name = field.name();
        quote! { ::sofie::db::read_columns::<#base>(&[#name]) }
    });
    let members = fields.iter().map(|field| field.ident);
    let types: Vec<_> = fields.iter().map(|field| field.ty).collect();
    quote! {
        impl ::sofie::db::Source for #ident {
            type Base = #base;

            fn columns() -> ::std::vec::Vec<::sofie::db::Selected> {
                [#(#selections),*].concat()
            }

            fn column_types() -> ::std::vec::Vec<::core::option::Option<::sofie::core::SqlType>> {
                ::sofie::db::column_types(&[#(<#types as ::sofie::core::Cast>::COLUMNS),*])
            }

            fn decode(row: &mut dyn ::sofie::core::ColumnReader) -> ::sofie::core::Result<Self> {
                ::core::result::Result::Ok(Self {
                    #(#members: <#types as ::sofie::core::Cast>::decode(row)?,)*
                })
            }
        }
    }
}

fn outcome_impl(ident: &Ident) -> TokenStream {
    quote! {
        impl ::sofie::db::Outcome for #ident {
            fn reply(self) -> ::sofie::core::Result<::sofie::db::Reply> {
                ::core::result::Result::Ok(::sofie::db::Reply::Data(::std::boxed::Box::new(::sofie::db::Showing(self))))
            }
        }

        impl ::sofie::db::ReadOnly for #ident {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expanded(input: &DeriveInput) -> String {
        derive(input).expect("the derive expands").to_string()
    }

    #[test]
    fn a_selected_field_must_not_be_sensitive() {
        let input: DeriveInput = syn::parse_quote! {
            #[read(from = Account)]
            struct Recovery { id: Id<Account>, recovery_code: Text }
        };
        let expanded = expanded(&input);

        assert!(
            expanded.contains("assert ! (readable (Account :: RECOVERY_CODE)"),
            "{expanded}"
        );
        assert!(!expanded.contains("readable (:: sofie"), "{expanded}");
    }

    #[test]
    fn a_field_marked_sensitive_selects_a_sensitive_field_on_purpose() {
        let input: DeriveInput = syn::parse_quote! {
            #[read(from = Account)]
            struct Recovery { id: Id<Account>, #[sensitive] recovery_code: Text, name: Text }
        };
        let expanded = expanded(&input);

        assert!(!expanded.contains("RECOVERY_CODE)"), "{expanded}");
        assert!(
            expanded.contains("readable (Account :: NAME)"),
            "{expanded}"
        );
    }

    #[test]
    fn a_field_marked_sensitive_is_masked_in_debug_output() {
        for input in [
            syn::parse_quote! {
                #[read(from = Account)]
                struct Recovery { id: Id<Account>, #[sensitive] recovery_code: Text }
            },
            syn::parse_quote! {
                struct Recovery { #[sensitive] recovery_code: Text }
            },
        ] {
            let expanded = expanded(&input);
            assert!(
                expanded.contains("impl :: core :: fmt :: Debug for Recovery"),
                "{expanded}"
            );
            assert!(expanded.contains("\"***\""), "{expanded}");
        }
    }

    #[test]
    fn a_computed_field_must_not_be_of_a_sensitive_type() {
        let input: DeriveInput = syn::parse_quote! {
            struct Payout { name: Text, iban: Iban, #[sensitive] code: Text }
        };
        let expanded = expanded(&input);

        assert!(
            expanded.contains("assert ! (! < SofieSensitivity < Iban >> :: SENSITIVE"),
            "{expanded}"
        );
        assert!(
            !expanded.contains("SofieSensitivity < Text >> :: SENSITIVE , \"`Payout.code`"),
            "{expanded}"
        );
    }

    #[test]
    fn a_computed_field_must_not_hold_a_sensitive_type() {
        let input: DeriveInput = syn::parse_quote! {
            struct Payouts {
                listed: Vec<Iban>,
                boxed: Box<[Iban]>,
                named: BTreeMap<Text, Arc<Iban>>,
                fixed: [(Text, Iban); 2],
            }
        };
        let expanded = expanded(&input);

        for field in ["listed", "boxed", "named", "fixed"] {
            assert!(
                expanded.contains(&format!(
                    "SofieSensitivity < Iban >> :: SENSITIVE , \"`Payouts.{field}`"
                )),
                "{field}: {expanded}"
            );
        }
    }

    #[cfg(feature = "joins")]
    #[test]
    fn a_joined_field_must_not_be_sensitive() {
        let input: DeriveInput = syn::parse_quote! {
            #[read(from = Order)]
            struct Row {
                id: Id<Order>,
                #[read(via = Order::CUSTOMER, field = Customer::EMAIL)]
                email: Option<Email>,
            }
        };
        let expanded = expanded(&input);

        assert!(
            expanded.contains("assert ! (readable (Customer :: EMAIL)"),
            "{expanded}"
        );
    }

    #[cfg(feature = "joins")]
    #[test]
    fn a_joined_field_is_checked_as_an_option_of_the_joined_column() {
        let input: DeriveInput = syn::parse_quote! {
            #[read(from = Order)]
            struct Row {
                id: Id<Order>,
                #[read(via = Order::CUSTOMER, field = Customer::NAME)]
                name: Option<String>,
            }
        };
        let expanded = expanded(&input);

        assert!(
            expanded.contains("joined_field :: < _ , _ , Option < String > > (Customer :: NAME)"),
            "{expanded}"
        );
    }
}
