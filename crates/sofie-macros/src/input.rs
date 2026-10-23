use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use syn::{Attribute, DeriveInput, Path, Type};

use crate::aggregate::rules_constant;
use crate::constraints::constraints;
use crate::shape::{NamedField, attributes_named, named_fields, resolve_impl, wrapped_type};

const UNCONSTRAINED: &str = "unconstrained";

pub(crate) fn derive(input: &DeriveInput) -> syn::Result<TokenStream> {
    let fields = named_fields(input, "Input")?;
    let options = Options::parse(&input.attrs)?;
    dto_impls(&input.ident, &fields, &options)
}

#[derive(Default)]
pub(crate) struct Options {
    from: Option<Path>,
    validate: bool,
}

impl Options {
    fn parse(attributes: &[Attribute]) -> syn::Result<Self> {
        let mut options = Self::default();
        for attribute in attributes_named(attributes, "input") {
            attribute.parse_nested_meta(|meta| {
                if meta.path.is_ident("from") {
                    options.from = Some(meta.value()?.parse()?);
                } else if meta.path.is_ident("validate") {
                    options.validate = true;
                } else {
                    return Err(meta.error("expected `from = Aggregate` or `validate`"));
                }
                Ok(())
            })?;
        }
        Ok(options)
    }
}

struct Rule(Option<TokenStream>);

impl Rule {
    fn of(field: &NamedField, from: Option<&Path>) -> syn::Result<Self> {
        let declared = constraints(field)?;
        if let Some(attribute) = attributes_named(field.attrs, UNCONSTRAINED).next() {
            attribute.meta.require_path_only()?;
            if declared.is_some() || from.is_none() {
                return Err(syn::Error::new_spanned(
                    attribute,
                    "`#[unconstrained]` drops what a field inherits through \
                     `#[input(from = …)]`; it stands alone",
                ));
            }
            return Ok(Self(None));
        }
        let ty = field.ty;
        Ok(Self(match (declared, from) {
            (Some(declared), _) => {
                Some(quote!(const { ::sofie::core::FieldRules::<#ty>::new(#declared) }))
            }
            (None, Some(aggregate)) => {
                let rules = rules_constant(field);
                Some(quote!(#aggregate::#rules))
            }
            (None, None) => None,
        }))
    }

    fn constraints(&self) -> TokenStream {
        self.0
            .as_ref()
            .map_or_else(|| quote!(&[]), |rules| quote!(#rules.constraints))
    }
}

pub(crate) fn dto_impls(
    ident: &Ident,
    fields: &[NamedField],
    options: &Options,
) -> syn::Result<TokenStream> {
    let rules = fields
        .iter()
        .map(|field| Rule::of(field, options.from.as_ref()))
        .collect::<syn::Result<Vec<_>>>()?;
    let input_impl = read_impl(ident, fields, &rules, options.validate);
    let form = form_fields_impl(ident, fields, &rules);
    #[cfg(feature = "fixtures")]
    let form = {
        let constraints: Vec<TokenStream> = rules.iter().map(Rule::constraints).collect();
        let fixture = crate::fixture::input_fixture(ident, fields, &constraints);
        quote!(#form #fixture)
    };
    let resolve = resolve_impl(
        ident,
        &quote!(::sofie::db::Writes),
        &quote! {
            ::core::future::ready(context.input().and_then(<Self as ::sofie::db::Input>::from_json))
        },
    );
    let names = field_names_impl(ident, fields);
    Ok(quote! { #input_impl #names #form #resolve })
}

fn field_names_impl(ident: &Ident, fields: &[NamedField]) -> TokenStream {
    let names = fields.iter().map(NamedField::name);
    quote! {
        impl ::sofie::db::FieldNames for #ident {
            const FIELD_NAMES: &'static [&'static str] = &[#(#names),*];
        }
    }
}

pub(crate) fn input_impl(ident: &Ident, fields: &[NamedField]) -> TokenStream {
    let rules: Vec<Rule> = fields.iter().map(|_| Rule(None)).collect();
    read_impl(ident, fields, &rules, false)
}

/// Reports all failures together, constraints included, before `Validate` runs.
fn read_impl(ident: &Ident, fields: &[NamedField], rules: &[Rule], validate: bool) -> TokenStream {
    let reads = fields
        .iter()
        .zip(rules)
        .map(|(field, rule)| read_field(field, rule));
    let members = fields.iter().map(|field| field.ident);
    let locals = fields.iter().map(local);
    let validate = validate.then(|| {
        quote! {
            <Self as ::sofie::db::Validate>::validate(&input).map_err(::sofie::core::Error::Invalid)?;
        }
    });
    quote! {
        impl ::sofie::db::Input for #ident {
            fn from_json(value: ::sofie::serde_json::Value) -> ::sofie::core::Result<Self> {
                let mut fields = ::sofie::db::InputFields::new(value)?;
                #(#reads)*
                fields.finish()?;
                let input = Self { #(#members: #locals?),* };
                #validate
                ::core::result::Result::Ok(input)
            }
        }
    }
}

pub(crate) fn take(field: &NamedField) -> TokenStream {
    let name = field.name();
    if is_option(field.ty) {
        return quote!(fields.optional(#name));
    }
    quote!(fields.take(#name))
}

fn read_field(field: &NamedField, rule: &Rule) -> TokenStream {
    let local = local(field);
    let take = take(field);
    let Some(rules) = &rule.0 else {
        return quote! { let #local = #take; };
    };
    let name = field.name();
    quote! {
        let #local = #take;
        let #local = fields.checked(#local, |value| #rules.refusal(#name, value));
    }
}

/// Higher-ranked bound: an input with a field no form shows still compiles, just not as a form.
fn form_fields_impl(ident: &Ident, fields: &[NamedField], rules: &[Rule]) -> TokenStream {
    let types: Vec<&Type> = fields.iter().map(|field| field.ty).collect();
    let descriptions = fields.iter().zip(rules).map(|(field, rule)| {
        let name = field.name();
        let ty = field.ty;
        let required = !is_option(ty);
        let constraints = rule.constraints();
        quote! {
            ::sofie::values::FormField {
                name: #name,
                input: <#ty as ::sofie::values::Present>::INPUT,
                required: #required,
                constraints: #constraints,
            }
        }
    });
    quote! {
        impl ::sofie::values::FormFields for #ident
        where
            #(for<'sofie> #types: ::sofie::values::Present,)*
        {
            const FIELDS: &'static [::sofie::values::FormField] = &[#(#descriptions),*];
        }
    }
}

/// Prefixed so that no field shadows the local `fields`.
pub(crate) fn local(field: &NamedField) -> Ident {
    format_ident!("field_{}", field.name())
}

pub(crate) fn is_option(ty: &Type) -> bool {
    wrapped_type(ty, "Option").is_some()
}
