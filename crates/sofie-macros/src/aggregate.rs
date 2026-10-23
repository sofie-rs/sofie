use proc_macro2::{Ident, TokenStream};
use quote::{quote, quote_spanned};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{DeriveInput, Meta, Path, Token, Type};

use crate::case::snake_case;
use crate::constraints::constraints;
use crate::shape::{
    NamedField, attributes_named, constant_of, named_fields, names_with, resolve_impl, wrapped_type,
};

/// Matched without a path, so `Default` does not shadow the standard trait.
const FRAMEWORK_BEHAVIORS: [&str; 10] = [
    "SoftDelete",
    "Active",
    "Default",
    "Slugged",
    "UniqueCode",
    "Audited",
    "RecordsChanges",
    "PublishLock",
    "Broadcasts",
    "Searched",
];

pub(crate) fn derive(input: &DeriveInput) -> syn::Result<TokenStream> {
    let fields = named_fields(input, "Aggregate")?;
    let sensitive = crate::sensitive::items(&input.ident, &fields);
    let (meta, fields): (Vec<NamedField>, Vec<NamedField>) =
        fields.into_iter().partition(|field| field.ident == "meta");
    let Some(meta) = meta.first() else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "an aggregate needs a `meta: Meta` field",
        ));
    };
    let (children, stored): (Vec<NamedField>, Vec<NamedField>) = fields
        .into_iter()
        .partition(|field| child_type(field.ty).is_some());
    refuse_reserved_names(&stored)?;
    let ident = &input.ident;
    let owner = owner_of(input, &stored)?;
    let behaviors = behaviors(input)?;
    #[cfg(feature = "contracts")]
    let (exposed, mirror) = (
        crate::contracts::exposed_items(&stored)?,
        crate::contracts::mirror_impl(input, &stored)?,
    );
    #[cfg(not(feature = "contracts"))]
    let (exposed, mirror) = (TokenStream::new(), TokenStream::new());
    #[cfg(feature = "retention")]
    let exposed = {
        let pruned = crate::pruned::pruned_constant(input);
        quote!(#exposed #pruned)
    };
    let aggregate = aggregate_impl(
        ident,
        meta,
        &stored,
        &children,
        owner.as_ref(),
        &behaviors,
        &exposed,
    );
    let uses = behaviors.iter().map(|behavior| {
        let path = &behavior.path;
        quote! { impl ::sofie::db::Uses<#path> for #ident {} }
    });
    let resolve = resolve_impl(
        ident,
        &quote!(::sofie::db::Writes),
        &quote!(::sofie::db::bind::<Self, SofieContext>(context)),
    );
    let columns = column_constants(ident, &stored)?;
    let across = across_constructor(ident);
    let unique_columns = unique_columns_check(&stored);
    #[cfg(feature = "children")]
    let parent_columns =
        crate::children::parent_column_checks(&snake_case(&ident.to_string()), &children);
    #[cfg(not(feature = "children"))]
    let parent_columns = TokenStream::new();
    #[cfg(feature = "factories")]
    let factory = crate::factory::default_factory(input, meta, &stored, &children, owner.as_ref())?;
    #[cfg(not(feature = "factories"))]
    let factory = TokenStream::new();
    Ok(
        quote! { #aggregate #(#uses)* #resolve #columns #across #unique_columns #parent_columns #factory #mirror #sensitive },
    )
}
/// The record's id and the columns of `Versioned` and `Timestamps`, which every table has.
const RESERVED_COLUMNS: [&str; 4] = ["id", "version", "created_at", "updated_at"];

fn refuse_reserved_names(fields: &[NamedField]) -> syn::Result<()> {
    for field in fields {
        let name = field.name();
        if RESERVED_COLUMNS.contains(&name.as_str()) {
            return Err(syn::Error::new_spanned(
                field.ident,
                format!("`{name}` is a column every aggregate already has; rename the field"),
            ));
        }
    }
    Ok(())
}

/// Column names are known only once types are, so the check runs at compile time: `total: Money`
/// stores `total_currency`, which a field of that name would store again.
pub(crate) fn unique_columns_check(fields: &[NamedField]) -> TokenStream {
    let columns = fields.iter().map(|field| {
        let name = field.name();
        let ty = field.ty;
        quote!((#name, <#ty as ::sofie::core::Cast>::COLUMNS))
    });
    let checks = fields.iter().enumerate().skip(1).map(|(index, field)| {
        let message = format!(
            "`{}` stores a column an earlier field already stores; rename it",
            field.name()
        );
        quote_spanned! {field.ident.span()=>
            ::core::assert!(!::sofie::core::stores_earlier_column(COLUMNS, #index), #message);
        }
    });
    quote! {
        const _: () = {
            const COLUMNS: &[(&str, &[::sofie::core::ColumnSpec])] = &[#(#columns),*];
            #(#checks)*
        };
    }
}

struct NamedBehavior {
    path: Path,
    arguments: Vec<(Ident, Ident)>,
}

impl NamedBehavior {
    fn parse(meta: &Meta) -> syn::Result<Self> {
        let mut arguments = Vec::new();
        if let Meta::List(list) = meta {
            list.parse_nested_meta(|argument| {
                let key = argument.path.require_ident()?.clone();
                arguments.push((key, argument.value()?.parse::<Ident>()?));
                Ok(())
            })?;
        } else {
            meta.require_path_only()?;
        }
        Ok(Self {
            path: qualified(meta.path()),
            arguments,
        })
    }

    fn value(&self, aggregate: &Ident, fields: &[NamedField]) -> TokenStream {
        let path = &self.path;
        if path
            .segments
            .last()
            .is_some_and(|last| last.ident == "Searched")
        {
            return searched(path, fields);
        }
        if self.arguments.is_empty() {
            return quote! { &#path };
        }
        let arguments = self.arguments.iter().map(|(key, field)| {
            let constant = constant_of(field);
            quote! { #key: ::sofie::db::Column::name(#aggregate::#constant) }
        });
        quote! { &#path { #(#arguments),* } }
    }
}

/// Each field with how its value is searched, which the behavior reads as each record is written.
/// Sensitive fields, by attribute or type, stay out, so no search matches or sends them.
fn searched(path: &Path, fields: &[NamedField]) -> TokenStream {
    let searchable = fields
        .iter()
        .filter(|field| !field.has_attribute("sensitive"));
    let entries = searchable.map(|field| {
        let name = field.name();
        let ty = field.ty;
        quote! {
            (
                #name,
                <#ty as ::sofie::core::Cast>::COLUMNS,
                if <#ty as ::sofie::core::Cast>::SENSITIVE {
                    ::sofie::values::Search::Skipped
                } else {
                    <#ty as ::sofie::values::Present>::SEARCH
                },
            )
        }
    });
    quote! { &#path { fields: &[#(#entries),*] } }
}

fn qualified(path: &Path) -> Path {
    match path.get_ident() {
        Some(ident) if FRAMEWORK_BEHAVIORS.contains(&ident.to_string().as_str()) => {
            syn::parse_quote!(::sofie::db::#ident)
        }
        _ => path.clone(),
    }
}

/// `Versioned` comes first in every aggregate without being named.
fn behaviors(input: &DeriveInput) -> syn::Result<Vec<NamedBehavior>> {
    let mut behaviors = vec![NamedBehavior {
        path: syn::parse_quote!(::sofie::db::Timestamps),
        arguments: Vec::new(),
    }];
    for attribute in attributes_named(&input.attrs, "aggregate") {
        attribute.parse_nested_meta(|meta| {
            if !meta.path.is_ident("behaviors") {
                return Err(meta.error("expected `behaviors(…)`"));
            }
            let content;
            syn::parenthesized!(content in meta.input);
            for named in Punctuated::<Meta, Token![,]>::parse_terminated(&content)? {
                refuse_default_behavior(named.path())?;
                behaviors.push(NamedBehavior::parse(&named)?);
            }
            Ok(())
        })?;
    }
    Ok(behaviors)
}

fn refuse_default_behavior(named: &Path) -> syn::Result<()> {
    let is_default = named
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "Versioned" || segment.ident == "Timestamps");
    if is_default {
        return Err(syn::Error::new_spanned(
            named,
            "`Versioned` and `Timestamps` are on by default; leave them out",
        ));
    }
    Ok(())
}

pub(crate) fn child_type(ty: &Type) -> Option<&Type> {
    wrapped_type(ty, "Children")
}

pub(crate) enum Owner<'f, 'a> {
    Field(&'f NamedField<'a>),
    Itself,
    #[cfg(feature = "children")]
    Through(Ident),
}

fn owner_of<'f, 'a>(
    input: &DeriveInput,
    fields: &'f [NamedField<'a>],
) -> syn::Result<Option<Owner<'f, 'a>>> {
    let on_struct = attributes_named(&input.attrs, "owner").next();
    match (on_struct, owner_field(fields)?) {
        (Some(attr), Some(_)) => Err(syn::Error::new_spanned(
            attr,
            "an aggregate is its own owner, owned through its parent or has an `#[owner]` \
             field, only one of them",
        )),
        (Some(attr), None) => struct_owner(&attr.meta).map(Some),
        (None, field) => Ok(field.map(Owner::Field)),
    }
}

fn struct_owner<'f, 'a>(meta: &Meta) -> syn::Result<Owner<'f, 'a>> {
    if matches!(meta, Meta::Path(_)) {
        return Ok(Owner::Itself);
    }
    #[cfg(feature = "children")]
    return crate::children::parent_field(meta).map(Owner::Through);
    #[cfg(not(feature = "children"))]
    Err(syn::Error::new_spanned(
        meta,
        "ownership through a parent needs the framework's `children` feature",
    ))
}

/// At most one field names the owner, so a record has one owner to scope by.
fn owner_field<'f, 'a>(fields: &'f [NamedField<'a>]) -> syn::Result<Option<&'f NamedField<'a>>> {
    let mut marked = fields.iter().filter(|field| field.has_attribute("owner"));
    let owner = marked.next();
    if let Some(second) = marked.next() {
        return Err(syn::Error::new_spanned(
            second.ident,
            "an aggregate has at most one `#[owner]` field",
        ));
    }
    Ok(owner)
}

/// The compiler checks the owner is an `Id<…>` at the field's type, a parent's at `owned_through`.
fn owner_items(owner: Option<&Owner>) -> TokenStream {
    let (name, uuid) = match owner {
        None => return TokenStream::new(),
        Some(Owner::Itself) => (
            "id".to_owned(),
            quote!(::sofie::db::Aggregate::meta(self).id()),
        ),
        #[cfg(feature = "children")]
        Some(Owner::Through(parent)) => return crate::children::through_items(parent),
        Some(Owner::Field(field)) => {
            let member = field.ident;
            let id = quote_spanned!(field.ty.span() => ::sofie::db::Id<_>);
            (
                field.name(),
                quote!({ let owner: #id = self.#member; owner.uuid() }),
            )
        }
    };
    quote! {
        const OWNER: ::core::option::Option<&'static str> = ::core::option::Option::Some(#name);

        fn owner(&self) -> ::core::option::Option<::sofie::db::Uuid> {
            ::core::option::Option::Some(#uuid)
        }
    }
}

fn aggregate_impl(
    ident: &Ident,
    meta: &NamedField,
    fields: &[NamedField],
    children: &[NamedField],
    owner: Option<&Owner>,
    behaviors: &[NamedBehavior],
    exposed: &TokenStream,
) -> TokenStream {
    let owner = owner_items(owner);
    #[cfg(feature = "children")]
    let children_items = crate::children::children_items(children);
    #[cfg(not(feature = "children"))]
    let children_items = TokenStream::new();
    let names: Vec<String> = fields.iter().map(NamedField::name).collect();
    let name = snake_case(&ident.to_string());
    let descriptions = fields.iter().map(field_description);
    let unique = names_with(fields, "unique");
    let sensitive = fields.iter().map(|field| {
        let (name, ty) = (field.name(), field.ty);
        let marked = field.has_attribute("sensitive");
        quote!((#name, #marked || <#ty as ::sofie::core::Cast>::SENSITIVE))
    });
    let behaviors = behaviors
        .iter()
        .map(|behavior| behavior.value(ident, fields));
    let members: Vec<&Ident> = fields.iter().map(|field| field.ident).collect();
    let types: Vec<_> = fields.iter().map(|field| field.ty).collect();
    let child_members = children.iter().map(|field| field.ident);
    let meta_ref = quote_spanned!(meta.ty.span() => &self.meta);
    let meta_mut = quote_spanned!(meta.ty.span() => &mut self.meta);
    let meta_init = quote_spanned!(meta.ty.span() => meta: loaded);
    quote! {
        impl ::sofie::db::Aggregate for #ident {
            const NAME: &'static str = #name;
            const MODULE: &'static str = ::core::env!("CARGO_CRATE_NAME");
            const FIELDS: &'static [::sofie::db::Field] = &[#(#descriptions),*];
            const UNIQUE: &'static [&'static str] = &[#(#unique),*];
            const SENSITIVE: &'static [&'static str] = {
                const FIELDS: &[(&str, bool)] = &[#(#sensitive),*];
                const NAMES: [&str; ::sofie::db::sensitive_count(FIELDS)] =
                    ::sofie::db::sensitive_names(FIELDS);
                &NAMES
            };
            const BEHAVIORS: &'static [&'static dyn ::sofie::db::Behavior] = &[#(#behaviors),*];
            const CHANGED: ::core::option::Option<&'static str> = ::core::option::Option::Some(::core::concat!(::core::env!("CARGO_CRATE_NAME"), ".", ::core::stringify!(#ident), "Changed"));
            #owner
            #children_items
            #exposed

            fn meta(&self) -> &::sofie::db::Meta {
                #meta_ref
            }

            fn meta_mut(&mut self) -> &mut ::sofie::db::Meta {
                #meta_mut
            }

            fn sensitive_json(&self) -> ::sofie::serde_json::Map<::std::string::String, ::sofie::serde_json::Value> {
                #[allow(unused_imports)]
                use ::sofie::db::{WritesJson as _, WritesNoJson as _};
                let mut fields = ::sofie::serde_json::Map::new();
                #(
                    if <#types as ::sofie::core::Cast>::SENSITIVE
                        && let ::core::option::Option::Some(value) = (&&::sofie::db::JsonOf(&self.#members)).json()
                    {
                        fields.insert(::std::string::String::from(#names), value);
                    }
                )*
                fields
            }

            fn encode(&self, out: &mut ::std::vec::Vec<::sofie::core::SqlValue>) {
                #(<#types as ::sofie::core::Cast>::encode(&self.#members, out);)*
            }

            fn decode(
                loaded: ::sofie::db::Meta,
                row: &mut dyn ::sofie::core::ColumnReader,
            ) -> ::sofie::core::Result<Self> {
                ::core::result::Result::Ok(Self {
                    #meta_init,
                    #(#members: <#types as ::sofie::core::Cast>::decode(row)?,)*
                    #(#child_members: ::sofie::db::Children::decode(row)?,)*
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
    }
}

pub(crate) fn field_description(field: &NamedField) -> TokenStream {
    let name = field.name();
    let ty = field.ty;
    // A `#[sensitive]` field is left out of the record's JSON, yet `sensitive_json` holds it when
    // its type is sensitive and serializes.
    let held = if field.has_attribute("sensitive") {
        quote!(<#ty as ::sofie::core::Cast>::SENSITIVE &&)
    } else {
        quote!(!<#ty as ::sofie::core::Cast>::SENSITIVE ||)
    };
    let handed_back = quote!(.handed_back_by(|| {
        use ::sofie::db::{Serializes as _, SerializesNot as _};
        #held (&&::sofie::db::JsonProbe::<#ty>(::core::marker::PhantomData)).serializes()
    }));
    quote! {
        ::sofie::db::Field::new(
            #name,
            <#ty as ::sofie::core::Cast>::COLUMNS,
            <#ty as ::sofie::core::Cast>::NULLABLE,
        )
        .parsed_by(|text| {
            use ::sofie::db::{ReadsNoText as _, ReadsText as _};
            (&&::sofie::db::TextOf::<#ty>(::core::marker::PhantomData)).read(text)
        })
        #handed_back
    }
}

/// Hidden beside each column: its constraint rules, which `#[input(from = …)]` inherits.
pub(crate) fn column_constants(ident: &Ident, fields: &[NamedField]) -> syn::Result<TokenStream> {
    let mut constants = Vec::new();
    for field in fields {
        let constant = field.constant();
        let rules = rules_constant(field);
        let name = field.name();
        let ty = field.ty;
        let field_rules = constraints(field)?.map_or_else(
            || quote!(::sofie::core::FieldRules::NONE),
            |constraints| quote!(::sofie::core::FieldRules::new(#constraints)),
        );
        constants.push(quote! {
            pub const #constant: ::sofie::db::Column<#ident, #ty> = ::sofie::db::Column::new(#name);
            #[doc(hidden)]
            pub const #rules: ::sofie::core::FieldRules<#ty> = #field_rules;
        });
    }
    Ok(quote! {
        impl #ident {
            #(#constants)*
        }
    })
}

/// Crate-private, so only the aggregate's own module checks across its owners.
fn across_constructor(ident: &Ident) -> TokenStream {
    quote! {
        impl #ident {
            #[allow(dead_code, reason = "generated for every aggregate, used by those that check across owners")]
            pub(crate) fn across(read: &::sofie::db::Read) -> ::sofie::db::Across<Self> {
                ::sofie::db::Across::for_derived_across_only(read)
            }
        }
    }
}

/// `__NAME`: no field's column constant is spelled so.
pub(crate) fn rules_constant(field: &NamedField) -> Ident {
    quote::format_ident!("__{}", field.constant())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_named_after_a_reserved_column_is_refused() {
        let input: DeriveInput = syn::parse_quote! {
            struct Order { meta: Meta, id: Text }
        };
        let error = derive(&input).expect_err("`id` is the record's own column");
        assert_eq!(
            error.to_string(),
            "`id` is a column every aggregate already has; rename the field"
        );
    }

    #[test]
    fn every_later_field_is_checked_against_the_columns_before_it() {
        let input: DeriveInput = syn::parse_quote! {
            struct Order { meta: Meta, total: Money, total_currency: Currency }
        };
        let expanded = derive(&input).expect("the derive expands").to_string();
        assert!(expanded.contains(
            "\"`total_currency` stores a column an earlier field already stores; rename it\""
        ));
        assert!(!expanded.contains("\"`total` stores a column"));
    }

    #[test]
    fn checking_across_owners_is_open_to_the_aggregate_s_own_crate_only() {
        let input: DeriveInput = syn::parse_quote! {
            struct Booking { meta: Meta, starts: Instant }
        };
        let expanded = derive(&input).expect("the derive expands").to_string();

        assert!(
            expanded.contains(
                "pub (crate) fn across (read : & :: sofie :: db :: Read) -> :: sofie :: db :: Across < Self >"
            ),
            "{expanded}"
        );
    }

    #[test]
    fn a_field_of_a_sensitive_type_counts_as_sensitive() {
        let input: DeriveInput = syn::parse_quote! {
            #[aggregate(behaviors(Searched))]
            struct Payee { meta: Meta, name: Text, iban: Iban }
        };
        let expanded = derive(&input).expect("the derive expands").to_string();
        let sensitive = expanded
            .split("const SENSITIVE")
            .nth(1)
            .and_then(|rest| rest.split("const BEHAVIORS").next())
            .expect("the aggregate lists its sensitive fields");

        assert!(
            sensitive.contains(
                "(\"iban\" , false || < Iban as :: sofie :: core :: Cast > :: SENSITIVE)"
            ),
            "{sensitive}"
        );
        assert!(
            expanded.contains("if < Iban as :: sofie :: core :: Cast > :: SENSITIVE"),
            "a field of a sensitive type is skipped by search: {expanded}"
        );
    }

    #[test]
    fn a_sensitive_field_is_kept_out_of_the_searched_text() {
        let input: DeriveInput = syn::parse_quote! {
            #[aggregate(behaviors(Searched))]
            struct Account { meta: Meta, name: Text, #[sensitive] recovery_code: Text }
        };
        let expanded = derive(&input).expect("the derive expands").to_string();
        let searched = expanded
            .split("Searched { fields")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .expect("the behavior lists its searched fields");

        assert!(searched.contains("\"name\""), "{searched}");
        assert!(!searched.contains("recovery_code"), "{searched}");
    }

    #[test]
    fn a_field_describes_how_its_type_reads_a_filter_s_text() {
        let input: DeriveInput = syn::parse_quote! {
            struct Tax { meta: Meta, rate: Percentage }
        };
        let fields = named_fields(&input, "Aggregate").unwrap();
        let description = field_description(&fields[1]).to_string();
        assert!(
            description.contains(
                "(&& :: sofie :: db :: TextOf :: < Percentage > (:: core :: marker :: PhantomData)) . read (text)"
            ),
            "{description}"
        );
    }
}
