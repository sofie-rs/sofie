//! The read model a `get` loads of the aggregate its path names, with the aggregate's fields.

use std::fmt::{self, Write as _};
use std::path::Path;

use quote::ToTokens;
use sofie_model::syntax::use_paths;
use sofie_model::{Aggregate, TypeName};
use syn::visit::Visit;
use syn::{Item, ItemStruct};

use crate::gen_command::GenError;
use crate::source_edits::read;

pub(super) struct ReadField {
    pub(super) name: String,
    /// As the aggregate spells it, so the derive's column check holds.
    ty: String,
    pub(super) money: bool,
    pub(super) instant: bool,
    sensitive: bool,
}

pub(super) struct ReadFields {
    pub(super) fields: Vec<ReadField>,
    /// `(use path, name)` pairs naming the fields' types where the read model is written.
    pub(super) imports: Vec<(String, String)>,
    pub(super) left_out: Vec<LeftOut>,
}

pub(super) struct LeftOut {
    field: String,
    reason: Omission,
}

enum Omission {
    Owner,
    Reference(String),
    Sensitive,
}

impl fmt::Display for LeftOut {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match &self.reason {
            Omission::Owner => "the record's owner, which the viewer knows".to_owned(),
            Omission::Reference(record) => {
                format!("it names a record of `{record}`, which the viewer may not reach")
            }
            Omission::Sensitive => "a secret, by its `#[sensitive]` mark or its type".to_owned(),
        };
        write!(
            formatter,
            "left out `{}`: {reason}; name it in `fields` to read it",
            self.field
        )
    }
}

impl ReadFields {
    pub(super) fn declarations(&self) -> String {
        let mut declarations = String::new();
        for field in &self.fields {
            if field.sensitive {
                declarations.push_str("    #[sensitive]\n");
            }
            let _ = writeln!(declarations, "    {}: {},", field.name, field.ty);
        }
        declarations
    }
}

/// The aggregate's module comes from its file, as the model records only modules declared inline.
pub(super) fn aggregate_module(source: &Path, aggregate: &Aggregate) -> Vec<String> {
    let relative = aggregate
        .file
        .strip_prefix(source)
        .unwrap_or(&aggregate.file)
        .with_extension("");
    let from_file = relative
        .iter()
        .map(|segment| segment.to_string_lossy().into_owned())
        .filter(|segment| segment != "mod" && segment != "lib");
    let inline = aggregate.path[..aggregate.path.len().saturating_sub(1)]
        .iter()
        .cloned();
    std::iter::once("crate".to_owned())
        .chain(from_file)
        .chain(inline)
        .collect()
}

/// The `chosen` fields, or by default every stored field a page may show: not one that is
/// sensitive, the owner or another record's id. A chosen sensitive field is marked `#[sensitive]`
/// as the derive requires. `sensitive_values` are the app's.
pub(super) fn read_fields(
    source: &Path,
    aggregate: &Aggregate,
    chosen: Option<&[String]>,
    sensitive_values: &[String],
) -> Result<ReadFields, GenError> {
    let text = read(&aggregate.file)?;
    let file = syn::parse_file(&text)
        .map_err(|error| GenError::Usage(format!("{}: {error}", aggregate.file.display())))?;
    let module = aggregate_module(source, aggregate);
    let scope = inline_scope(&file.items, &aggregate.path);
    let declared = scope.and_then(|items| declared_struct(items, &aggregate.name));
    let readable = readable_fields(aggregate, declared);
    let items = scope.unwrap_or_default();
    let (picked, left_out) = match chosen {
        None => shown_by_default(aggregate, &readable, items, sensitive_values),
        Some(chosen) => (
            picked_fields(&readable, chosen, &aggregate.name)?,
            Vec::new(),
        ),
    };
    let imports = type_imports(items, &module, &picked);
    let id = ReadField {
        name: "id".to_owned(),
        ty: format!("Id<{}>", aggregate.name),
        money: false,
        instant: false,
        sensitive: false,
    };
    let fields = std::iter::once(id)
        .chain(picked.iter().map(|(name, declared)| ReadField {
            name: (*name).to_owned(),
            ty: declared.ty.to_token_stream().to_string(),
            money: type_names(&declared.ty) == ["Money"],
            instant: is_instant(&declared.ty),
            sensitive: is_sensitive(declared, sensitive_values),
        }))
        .collect();
    Ok(ReadFields {
        fields,
        imports,
        left_out,
    })
}

type Readable<'model> = (&'model str, &'model syn::Field);

fn shown_by_default<'model>(
    aggregate: &Aggregate,
    readable: &[Readable<'model>],
    items: &[Item],
    sensitive_values: &[String],
) -> (Vec<Readable<'model>>, Vec<LeftOut>) {
    let mut shown = Vec::new();
    let mut left_out = Vec::new();
    for &(name, declared) in readable {
        let reason = if aggregate.owner.as_deref() == Some(name) {
            Some(Omission::Owner)
        } else if is_sensitive(declared, sensitive_values) {
            Some(Omission::Sensitive)
        } else {
            aggregate
                .fields
                .iter()
                .find(|field| field.name == name)
                .and_then(|field| referenced_record(&field.ty, items))
                .map(Omission::Reference)
        };
        match reason {
            Some(reason) => left_out.push(LeftOut {
                field: name.to_owned(),
                reason,
            }),
            None => shown.push((name, declared)),
        }
    }
    (shown, left_out)
}

/// Through an import's alias too, as `use sofie_identity::AccountId as Payer`.
fn referenced_record(ty: &TypeName, items: &[Item]) -> Option<String> {
    if let Some(record) = ty.record() {
        return Some(record.to_owned());
    }
    let named = match (ty.name.as_str(), ty.arguments.as_slice()) {
        ("Option", [inner]) => inner,
        _ => ty,
    };
    let target = items
        .iter()
        .filter_map(|item| match item {
            Item::Use(used) => Some(use_paths(&used.tree)),
            _ => None,
        })
        .flatten()
        .find(|used| used.binding.as_deref() == Some(named.name.as_str()))?
        .segments
        .last()?
        .clone();
    TypeName {
        name: target,
        arguments: Vec::new(),
    }
    .record()
    .map(str::to_owned)
}

fn is_instant(ty: &syn::Type) -> bool {
    let names = type_names(ty);
    names == ["Instant"] || names == ["Option", "Instant"]
}

fn readable_fields<'model>(
    aggregate: &'model Aggregate,
    declared: Option<&'model ItemStruct>,
) -> Vec<Readable<'model>> {
    aggregate
        .fields
        .iter()
        .filter_map(|field| {
            let declared = declared?.fields.iter().find(|declared| {
                declared
                    .ident
                    .as_ref()
                    .is_some_and(|ident| *ident == field.name)
            })?;
            Some((field.name.as_str(), declared))
        })
        .collect()
}

fn type_imports(
    items: &[Item],
    module: &[String],
    fields: &[Readable<'_>],
) -> Vec<(String, String)> {
    let mut imports = Vec::new();
    for (_, field) in fields {
        for name in type_names(&field.ty) {
            if let Some(import) = import_of(items, module, &name)
                && !imports.contains(&import)
            {
                imports.push(import);
            }
        }
    }
    imports
}

fn picked_fields<'fields>(
    readable: &[Readable<'fields>],
    chosen: &[String],
    aggregate: &str,
) -> Result<Vec<Readable<'fields>>, GenError> {
    chosen
        .iter()
        .filter(|name| name.as_str() != "id")
        .map(|name| {
            readable
                .iter()
                .find(|(field, _)| field == name)
                .copied()
                .ok_or_else(|| {
                    let known: Vec<&str> = readable.iter().map(|(field, _)| *field).collect();
                    let fields = if known.is_empty() {
                        "it has no readable field besides `id`".to_owned()
                    } else {
                        format!("its fields: {}", known.join(", "))
                    };
                    GenError::Usage(format!(
                        "`{aggregate}` has no field `{name}` a read model can read; {fields}"
                    ))
                })
        })
        .collect()
}

/// By its attribute or its type: `Secret` or a value of the crate marked `#[sensitive]`.
fn is_sensitive(field: &syn::Field, sensitive_values: &[String]) -> bool {
    field
        .attrs
        .iter()
        .any(|attribute| attribute.path().is_ident("sensitive"))
        || type_names(&field.ty)
            .iter()
            .any(|name| name == "Secret" || sensitive_values.contains(name))
}

/// The items of the inline module the aggregate is declared in, within its file.
fn inline_scope<'items>(items: &'items [Item], path: &[String]) -> Option<&'items [Item]> {
    let Some((_, inline)) = path.split_last() else {
        return Some(items);
    };
    inline.iter().try_fold(items, |items, segment| {
        items.iter().find_map(|item| match item {
            Item::Mod(module) if module.ident == segment => {
                module.content.as_ref().map(|(_, items)| items.as_slice())
            }
            _ => None,
        })
    })
}

fn declared_struct<'items>(items: &'items [Item], name: &str) -> Option<&'items ItemStruct> {
    items.iter().find_map(|item| match item {
        Item::Struct(structure) if structure.ident == name => Some(structure),
        _ => None,
    })
}

/// Unqualified type names, as `Option<OrderStatus>` names `Option` and `OrderStatus`.
fn type_names(ty: &syn::Type) -> Vec<String> {
    struct Names(Vec<String>);
    impl<'ast> Visit<'ast> for Names {
        fn visit_type_path(&mut self, path: &'ast syn::TypePath) {
            if path.qself.is_none()
                && let [segment] = path.path.segments.iter().collect::<Vec<_>>()[..]
            {
                self.0.push(segment.ident.to_string());
            }
            syn::visit::visit_type_path(self, path);
        }
    }
    let mut names = Names(Vec::new());
    names.visit_type(ty);
    names.0
}

/// A name the aggregate's module declares or imports by name; one from a glob, as the prelude's, needs none.
fn import_of(items: &[Item], module: &[String], name: &str) -> Option<(String, String)> {
    if items.iter().any(|item| declares(item, name)) {
        return Some((use_path(module), name.to_owned()));
    }
    items
        .iter()
        .filter_map(|item| match item {
            Item::Use(used) => Some(use_paths(&used.tree)),
            _ => None,
        })
        .flatten()
        .find(|used| used.binding.as_deref() == Some(name))
        .and_then(|used| {
            let (target, parent) = used.segments.split_last()?;
            let parent = absolute(parent, module)?;
            let named = if target == name {
                name.to_owned()
            } else {
                format!("{target} as {name}")
            };
            Some((use_path(&parent), named))
        })
}

fn declares(item: &Item, name: &str) -> bool {
    match item {
        Item::Struct(item) => item.ident == name,
        Item::Enum(item) => item.ident == name,
        Item::Type(item) => item.ident == name,
        _ => false,
    }
}

/// `self::` and `super::` mean the aggregate's module, which the read model is not in.
fn absolute(segments: &[String], module: &[String]) -> Option<Vec<String>> {
    let mut resolved = module.to_vec();
    let mut rest = segments;
    match segments.first().map(String::as_str) {
        Some("self") => rest = &segments[1..],
        Some("super") => {
            while rest.first().is_some_and(|segment| segment == "super") {
                resolved.pop()?;
                rest = &rest[1..];
            }
        }
        Some(_) => return Some(segments.to_vec()),
        None => return None,
    }
    resolved.extend_from_slice(rest);
    Some(resolved)
}

fn use_path(segments: &[String]) -> String {
    format!("use {}", segments.join("::"))
}
