use std::fmt;
use std::path::{Path, PathBuf};

use syn::ext::IdentExt;
use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::{
    Attribute, Expr, ExprCall, ExprMethodCall, Fields, ImplItem, Item, ItemEnum, ItemFn, ItemImpl,
    ItemStruct, Macro, ReturnType, Token,
};

use crate::FeatureCode;
use crate::constraints::described;
use crate::effects::Effects;
use crate::entitlements::{EntitlementDeclaration, entitlement_of};
use crate::handlers::HandlerScope;
use crate::naming::{DEFAULT_BEHAVIORS, behavior_columns, stored_name};
use crate::syntax::{
    doc_line, last_name, line_of, macro_expressions, path_ends_with, segments, string_literal,
    type_last_name,
};
use crate::telemetry::TelemetryCode;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeName {
    /// Last segment; `()` for unit, `(…)` for a tuple, `&` for a reference, `_` for anything else.
    pub name: String,
    pub arguments: Vec<TypeName>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    pub path: Vec<String>,
    pub arguments: Vec<TypeName>,
    pub output: Option<TypeName>,
    pub renders: Vec<ScreenRender>,
    pub doc: Option<String>,
    pub effects: Effects,
    pub file: PathBuf,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeMethod {
    pub owner: String,
    pub name: String,
    /// A call of a trait's method by name does not show which impl runs.
    pub implements: Option<String>,
    pub doc: Option<String>,
    pub effects: Effects,
    pub file: PathBuf,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenRender {
    pub screen: Option<String>,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDeclaration {
    pub name: String,
    pub ty: TypeName,
    pub constraints: Vec<String>,
    /// Each with its bound: `max 120`, `required`.
    pub bounds: Vec<String>,
    pub line: usize,
}

/// Must match what `#[derive(Input)]` and `#[derive(Aggregate)]` read.
const CONSTRAINT_ATTRIBUTES: [&str; 6] = ["min", "max", "length", "range", "required", "unique"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnumDeclaration {
    pub name: String,
    /// From the crate's root, through a root re-export where there is one.
    pub path: Vec<String>,
    /// `PartlyShipped` is stored as `PARTLY_SHIPPED`.
    pub stored_names: Vec<String>,
    pub variants: Vec<EnumVariant>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnumVariant {
    pub name: String,
    pub to: Vec<String>,
    /// `#[tone(success)]`: how a badge shows it.
    pub tone: Option<String>,
}

impl EnumDeclaration {
    #[must_use]
    pub fn has_transitions(&self) -> bool {
        self.variants.iter().any(|variant| !variant.to.is_empty())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadModel {
    pub name: String,
    pub from: Option<String>,
    pub fields: Vec<FieldDeclaration>,
    pub file: PathBuf,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Aggregate {
    pub name: String,
    pub path: Vec<String>,
    pub mirror: Option<Vec<String>>,
    /// Stored columns; `Children` fields live in their own tables and are in `children`.
    pub fields: Vec<FieldDeclaration>,
    pub children: Vec<FieldDeclaration>,
    pub behaviors: Vec<String>,
    pub exposed: Vec<String>,
    pub owner: Option<String>,
    /// `#[owner]` on the struct: its own owner, or owned through its parent.
    pub owner_on_struct: bool,
    pub doc: Option<String>,
    pub file: PathBuf,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputDeclaration {
    pub name: String,
    pub path: Vec<String>,
    pub from: Option<String>,
    pub fields: Vec<FieldDeclaration>,
    pub unconstrained: Vec<String>,
    pub file: PathBuf,
    pub line: usize,
}

impl InputDeclaration {
    #[must_use]
    pub fn inherited<'a>(&self, field: &str, code: &'a Code) -> Option<&'a FieldDeclaration> {
        let from = self.from.as_deref()?;
        code.aggregates
            .iter()
            .find(|aggregate| aggregate.name == from)?
            .fields
            .iter()
            .find(|declared| declared.name == field)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawSql {
    pub module_path: Vec<String>,
    pub file: PathBuf,
    pub line: usize,
    /// The statement when written as a literal, with the line it starts on.
    pub text: Option<(String, usize)>,
}

/// A translation key Rust code names as a literal, which the source language must have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyLiteral {
    pub key: String,
    pub file: PathBuf,
    pub line: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Code {
    pub functions: Vec<Function>,
    pub methods: Vec<TypeMethod>,
    pub read_models: Vec<ReadModel>,
    pub aggregates: Vec<Aggregate>,
    pub inputs: Vec<InputDeclaration>,
    pub enums: Vec<EnumDeclaration>,
    pub raw_sql: Vec<RawSql>,
    pub translation_keys: Vec<KeyLiteral>,
    pub features: FeatureCode,
    pub entitlements: Vec<EntitlementDeclaration>,
    pub telemetry: TelemetryCode,
}

impl Code {
    #[must_use]
    pub fn function(&self, path: &[String]) -> Option<&Function> {
        self.functions.iter().find(|function| function.path == path)
    }

    #[must_use]
    pub fn read_model(&self, name: &str) -> Option<&ReadModel> {
        self.read_models.iter().find(|model| model.name == name)
    }

    #[must_use]
    pub fn input_of(&self, function: &Function) -> Option<&InputDeclaration> {
        function.arguments.iter().find_map(|argument| {
            self.inputs
                .iter()
                .find(|input| argument.arguments.is_empty() && input.name == argument.name)
        })
    }

    /// The input a GET reads from its query string, as `Query<Q>` or `Listing<T, Q>`.
    #[must_use]
    pub fn query_of(&self, function: &Function) -> Option<&InputDeclaration> {
        function.arguments.iter().find_map(|argument| {
            let (("Query", [parameters]) | ("Listing", [_, parameters])) =
                (argument.name.as_str(), argument.arguments.as_slice())
            else {
                return None;
            };
            self.inputs
                .iter()
                .find(|input| parameters.arguments.is_empty() && input.name == parameters.name)
        })
    }

    /// A contract cannot name the module's own types: an `Id<X>` becomes its uuid, a module enum its stored name.
    pub(crate) fn contract_type(&self, ty: &TypeName) -> Option<String> {
        let name = ty.name.as_str();
        match ty.arguments.as_slice() {
            [inner] if name == "Option" => Some(format!("Option<{}>", self.contract_type(inner)?)),
            [_] if name == "Id" => Some("sofie::db::Uuid".to_owned()),
            [] if self.enums.iter().any(|declared| declared.name == name) => {
                Some("String".to_owned())
            }
            [] if CONTRACT_VALUES.contains(&name) => Some(format!("sofie::values::{name}")),
            [] if CONTRACT_SCALARS.contains(&name) => Some(name.to_owned()),
            _ => None,
        }
    }

    pub(crate) fn collect(&mut self, item: &Item, file: &Path, scope: &HandlerScope) {
        let module_path = scope.module_path();
        self.features.collect(item, file, module_path);
        self.entitlements
            .extend(entitlement_of(item, file, module_path));
        self.telemetry.collect(item, file, module_path);
        let mut calls = SqlAndKeyCalls {
            module_path,
            file,
            code: self,
        };
        calls.visit_item(item);
        match item {
            Item::Fn(function) => self.functions.push(function_of(function, file, scope)),
            Item::Impl(implementation) => {
                self.methods.extend(methods_of(implementation, file, scope));
            }
            Item::Struct(structure) if derives(&structure.attrs, "ReadModel") => {
                self.read_models.push(read_model_of(structure, file));
            }
            Item::Struct(structure) if derives(&structure.attrs, "Aggregate") => {
                self.aggregates
                    .push(aggregate_of(structure, file, module_path));
            }
            Item::Struct(structure) if derives(&structure.attrs, "Input") => {
                self.inputs.push(input_of(structure, file, module_path));
            }
            Item::Enum(enumeration) if derives(&enumeration.attrs, "Enum") => {
                self.enums.push(enum_of(enumeration, module_path));
            }
            _ => {}
        }
    }
}

const CONTRACT_VALUES: [&str; 19] = [
    "Coordinate",
    "Currency",
    "Date",
    "Email",
    "Energy",
    "Instant",
    "Length",
    "LocalDateTime",
    "Locale",
    "Money",
    "Percentage",
    "PersonName",
    "PhoneNumber",
    "Slug",
    "TimeOfDay",
    "TimeZone",
    "Url",
    "Weekday",
    "Weight",
];

const CONTRACT_SCALARS: [&str; 7] = ["String", "bool", "i16", "i32", "i64", "u16", "u32"];

impl Aggregate {
    pub fn contract_fields(&self, code: &Code) -> Result<Vec<(String, String)>, &FieldDeclaration> {
        self.fields
            .iter()
            .filter(|field| self.exposed.contains(&field.name))
            .map(|field| match code.contract_type(&field.ty) {
                Some(ty) => Ok((field.name.clone(), ty)),
                None => Err(field),
            })
            .collect()
    }

    pub fn all_behaviors(&self) -> impl Iterator<Item = &str> {
        DEFAULT_BEHAVIORS
            .into_iter()
            .chain(self.behaviors.iter().map(String::as_str))
    }

    #[must_use]
    pub fn is_owned(&self) -> bool {
        self.owner.is_some() || self.owner_on_struct
    }

    #[must_use]
    pub fn soft_deletes(&self) -> bool {
        self.behaviors
            .iter()
            .any(|behavior| behavior == "SoftDelete")
    }

    #[must_use]
    pub fn columns(&self) -> Vec<String> {
        let behaviors = self.all_behaviors().flat_map(behavior_columns);
        self.fields
            .iter()
            .map(|field| field.name.clone())
            .chain(behaviors.map(|column| column.name.to_owned()))
            .collect()
    }
}

impl TypeName {
    /// `syn` cannot see through id aliases, so `AccountId` names `Account` by naming convention.
    #[must_use]
    pub fn record(&self) -> Option<&str> {
        match (self.name.as_str(), self.arguments.as_slice()) {
            ("Option", [inner]) => inner.record(),
            ("Id", [target]) => Some(&target.name),
            (name, []) => name
                .strip_suffix("Id")
                .filter(|stem| stem.starts_with(char::is_uppercase)),
            _ => None,
        }
    }

    #[must_use]
    pub fn without_result(&self) -> &Self {
        match (self.name.as_str(), self.arguments.first()) {
            ("Result", Some(inner)) => inner,
            _ => self,
        }
    }

    pub(crate) fn of(ty: &syn::Type) -> Self {
        let (name, arguments) = match ty {
            syn::Type::Path(path) if path.qself.is_none() => return Self::of_path(&path.path),
            syn::Type::Tuple(tuple) if tuple.elems.is_empty() => ("()", Vec::new()),
            syn::Type::Tuple(tuple) => ("(…)", tuple.elems.iter().map(Self::of).collect()),
            syn::Type::Reference(reference) => ("&", vec![Self::of(&reference.elem)]),
            syn::Type::Paren(inner) => return Self::of(&inner.elem),
            syn::Type::TraitObject(object) => ("dyn", trait_bounds(&object.bounds)),
            _ => ("_", Vec::new()),
        };
        Self {
            name: name.to_owned(),
            arguments,
        }
    }

    fn of_path(path: &syn::Path) -> Self {
        let Some(last) = path.segments.last() else {
            return Self {
                name: "_".to_owned(),
                arguments: Vec::new(),
            };
        };
        let arguments = match &last.arguments {
            syn::PathArguments::AngleBracketed(bracketed) => bracketed
                .args
                .iter()
                .filter_map(|argument| match argument {
                    syn::GenericArgument::Type(ty) => Some(Self::of(ty)),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        Self {
            name: last.ident.unraw().to_string(),
            arguments,
        }
    }
}

impl fmt::Display for TypeName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let arguments: Vec<String> = self.arguments.iter().map(ToString::to_string).collect();
        match self.name.as_str() {
            "(…)" => write!(formatter, "({})", arguments.join(", ")),
            "&" => write!(formatter, "&{}", arguments.join("")),
            "dyn" => write!(formatter, "dyn {}", arguments.join(" + ")),
            name if arguments.is_empty() => formatter.write_str(name),
            name => write!(formatter, "{name}<{}>", arguments.join(", ")),
        }
    }
}

fn trait_bounds(bounds: &Punctuated<syn::TypeParamBound, Token![+]>) -> Vec<TypeName> {
    bounds
        .iter()
        .filter_map(|bound| match bound {
            syn::TypeParamBound::Trait(bound) => Some(TypeName::of_path(&bound.path)),
            _ => None,
        })
        .collect()
}

fn function_of(function: &ItemFn, file: &Path, scope: &HandlerScope) -> Function {
    let mut path = scope.module_path().to_vec();
    path.push(function.sig.ident.to_string());
    let mut renders = ScreenRenders::default();
    renders.visit_block(&function.block);
    let arguments = function
        .sig
        .inputs
        .iter()
        .filter_map(|argument| match argument {
            syn::FnArg::Typed(typed) => Some(TypeName::of(&typed.ty)),
            syn::FnArg::Receiver(_) => None,
        })
        .collect();
    Function {
        path,
        arguments,
        output: match &function.sig.output {
            ReturnType::Default => None,
            ReturnType::Type(_, ty) => Some(TypeName::of(ty)),
        },
        renders: renders.found,
        doc: doc_line(&function.attrs),
        effects: Effects::of(&function.sig, &function.block, None, scope),
        file: file.to_path_buf(),
        line: line_of(&function.sig.ident),
    }
}

fn methods_of(implementation: &ItemImpl, file: &Path, scope: &HandlerScope) -> Vec<TypeMethod> {
    let Some(owner) = type_last_name(&implementation.self_ty) else {
        return Vec::new();
    };
    let implements = implementation
        .trait_
        .as_ref()
        .and_then(|(.., path, _)| last_name(path));
    implementation
        .items
        .iter()
        .filter_map(|item| match item {
            ImplItem::Fn(method) => Some(TypeMethod {
                owner: owner.clone(),
                name: method.sig.ident.unraw().to_string(),
                implements: implements.clone(),
                doc: doc_line(&method.attrs),
                effects: Effects::of(&method.sig, &method.block, Some(&owner), scope),
                file: file.to_path_buf(),
                line: line_of(&method.sig.ident),
            }),
            _ => None,
        })
        .collect()
}

fn enum_of(enumeration: &ItemEnum, module_path: &[String]) -> EnumDeclaration {
    let variants: Vec<EnumVariant> = enumeration
        .variants
        .iter()
        .map(|variant| EnumVariant {
            name: variant.ident.unraw().to_string(),
            tone: variant
                .attrs
                .iter()
                .filter(|attribute| attribute.path().is_ident("tone"))
                .find_map(|attribute| attribute.parse_args::<syn::Ident>().ok())
                .map(|tone| tone.to_string()),
            to: variant
                .attrs
                .iter()
                .filter(|attribute| attribute.path().is_ident("to"))
                .filter_map(|attribute| {
                    attribute
                        .parse_args_with(Punctuated::<syn::Ident, Token![,]>::parse_terminated)
                        .ok()
                })
                .flatten()
                .map(|target| target.unraw().to_string())
                .collect(),
        })
        .collect();
    let name = enumeration.ident.unraw().to_string();
    EnumDeclaration {
        path: [module_path, std::slice::from_ref(&name)].concat(),
        name,
        stored_names: variants
            .iter()
            .map(|variant| stored_name(&variant.name))
            .collect(),
        variants,
    }
}

fn read_model_of(structure: &ItemStruct, file: &Path) -> ReadModel {
    ReadModel {
        name: structure.ident.unraw().to_string(),
        from: from_argument(&structure.attrs, "read"),
        fields: fields_of(&structure.fields, |_| true),
        file: file.to_path_buf(),
        line: line_of(&structure.ident),
    }
}

fn from_argument(attributes: &[Attribute], attribute: &str) -> Option<String> {
    let mut from = None;
    for attribute in attributes
        .iter()
        .filter(|candidate| candidate.path().is_ident(attribute))
    {
        // A malformed attribute is the derive's to report; the model only reads.
        let _ = attribute.parse_nested_meta(|meta| {
            if meta.path.is_ident("from") {
                let path: syn::Path = meta.value()?.parse()?;
                from = last_name(&path);
            }
            Ok(())
        });
    }
    from
}

fn input_of(structure: &ItemStruct, file: &Path, module_path: &[String]) -> InputDeclaration {
    let name = structure.ident.unraw().to_string();
    let mut path = module_path.to_vec();
    path.push(name.clone());
    InputDeclaration {
        name,
        path,
        from: from_argument(&structure.attrs, "input"),
        fields: fields_of(&structure.fields, |_| true),
        unconstrained: fields_marked(&structure.fields, "unconstrained").collect(),
        file: file.to_path_buf(),
        line: line_of(&structure.ident),
    }
}

fn aggregate_of(structure: &ItemStruct, file: &Path, module_path: &[String]) -> Aggregate {
    let mut behaviors = Vec::new();
    for attribute in structure
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("aggregate"))
    {
        // A malformed attribute is the derive's to report; the model only reads.
        let _ = attribute.parse_nested_meta(|meta| {
            if meta.path.is_ident("behaviors") {
                let content;
                syn::parenthesized!(content in meta.input);
                let named = Punctuated::<syn::Meta, Token![,]>::parse_terminated(&content)?;
                behaviors.extend(
                    named
                        .iter()
                        .filter_map(|behavior| last_name(behavior.path())),
                );
            }
            Ok(())
        });
    }
    let name = structure.ident.unraw().to_string();
    let mirror = structure
        .attrs
        .iter()
        .find(|attribute| attribute.path().is_ident("mirror"))
        .and_then(|attribute| attribute.parse_args::<syn::Path>().ok())
        .map(|path| segments(&path));
    let (children, fields) = fields_of(&structure.fields, |name| name != "meta")
        .into_iter()
        .partition(|field| field.ty.name == "Children");
    Aggregate {
        path: [module_path, std::slice::from_ref(&name)].concat(),
        name,
        mirror,
        fields,
        children,
        behaviors,
        exposed: fields_marked(&structure.fields, "expose").collect(),
        owner: fields_marked(&structure.fields, "owner").next(),
        owner_on_struct: structure
            .attrs
            .iter()
            .any(|attribute| attribute.path().is_ident("owner")),
        doc: doc_line(&structure.attrs),
        file: file.to_path_buf(),
        line: line_of(&structure.ident),
    }
}

fn fields_marked<'fields>(
    fields: &'fields Fields,
    attribute: &'fields str,
) -> impl Iterator<Item = String> + 'fields {
    fields
        .iter()
        .filter(move |field| {
            field
                .attrs
                .iter()
                .any(|candidate| candidate.path().is_ident(attribute))
        })
        .filter_map(|field| Some(field.ident.as_ref()?.unraw().to_string()))
}

fn fields_of(fields: &Fields, keep: impl Fn(&str) -> bool) -> Vec<FieldDeclaration> {
    fields
        .iter()
        .filter_map(|field| {
            let ident = field.ident.as_ref()?;
            let name = ident.unraw().to_string();
            let constrained: Vec<(&Attribute, String)> = field
                .attrs
                .iter()
                .filter_map(|attribute| {
                    let name = attribute.path().get_ident()?.to_string();
                    CONSTRAINT_ATTRIBUTES
                        .contains(&name.as_str())
                        .then_some((attribute, name))
                })
                .collect();
            keep(&name).then(|| FieldDeclaration {
                name,
                ty: TypeName::of(&field.ty),
                bounds: constrained
                    .iter()
                    .flat_map(|(attribute, name)| described(attribute, name))
                    .collect(),
                constraints: constrained.into_iter().map(|(_, name)| name).collect(),
                line: line_of(ident),
            })
        })
        .collect()
}

pub(crate) fn derives(attributes: &[Attribute], derive: &str) -> bool {
    attributes
        .iter()
        .filter(|attribute| attribute.path().is_ident("derive"))
        .any(|attribute| {
            let mut found = false;
            let parsed = attribute.parse_nested_meta(|meta| {
                found |= path_ends_with(&meta.path, &[derive]);
                Ok(())
            });
            parsed.is_ok() && found
        })
}

#[derive(Default)]
struct ScreenRenders {
    found: Vec<ScreenRender>,
}

impl Visit<'_> for ScreenRenders {
    fn visit_expr_call(&mut self, call: &ExprCall) {
        if let Expr::Path(function) = call.func.as_ref()
            && path_ends_with(&function.path, &["Screen", "render"])
        {
            self.found.push(ScreenRender {
                screen: call.args.first().and_then(string_literal),
                line: line_of(call),
            });
        }
        visit::visit_expr_call(self, call);
    }

    fn visit_macro(&mut self, call: &Macro) {
        for expression in macro_expressions(call) {
            self.visit_expr(&expression);
        }
    }
}

/// `.sql(…)` calls, and `FieldError::new(field, "key", message)`, whose second argument is a key.
struct SqlAndKeyCalls<'code> {
    module_path: &'code [String],
    file: &'code Path,
    code: &'code mut Code,
}

impl Visit<'_> for SqlAndKeyCalls<'_> {
    fn visit_expr_call(&mut self, call: &ExprCall) {
        if let Expr::Path(function) = call.func.as_ref()
            && path_ends_with(&function.path, &["FieldError", "new"])
            && let Some(key) = call.args.iter().nth(1)
            && let Some(text) = string_literal(key)
        {
            self.code.translation_keys.push(KeyLiteral {
                key: text,
                file: self.file.to_path_buf(),
                line: line_of(key),
            });
        }
        visit::visit_expr_call(self, call);
    }

    fn visit_expr_method_call(&mut self, call: &ExprMethodCall) {
        if call.method == "sql" {
            self.code.raw_sql.push(RawSql {
                module_path: self.module_path.to_vec(),
                file: self.file.to_path_buf(),
                line: line_of(&call.method),
                text: call
                    .args
                    .first()
                    .and_then(|first| Some((string_literal(first)?, line_of(first)))),
            });
        }
        visit::visit_expr_method_call(self, call);
    }

    fn visit_macro(&mut self, call: &Macro) {
        for expression in macro_expressions(call) {
            self.visit_expr(&expression);
        }
    }
}
