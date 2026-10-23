use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::TokenStream;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{
    Attribute, Expr, ExprMethodCall, FnArg, GenericArgument, Item, ItemFn, Lit, Macro, Pat,
    PathArguments, PathSegment, Token, Type, UseTree, Visibility,
};

use crate::ModelError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsePath {
    pub segments: Vec<String>,
    pub binding: Option<String>,
    pub line: usize,
}

/// A file outside the crate's module tree is not in `parsed`, yet the model still reads it.
pub(crate) fn parsed_or_read<'parsed>(
    parsed: &'parsed BTreeMap<PathBuf, syn::File>,
    file: &Path,
) -> Result<Cow<'parsed, syn::File>, ModelError> {
    match parsed.get(file) {
        Some(syntax) => Ok(Cow::Borrowed(syntax)),
        None => Ok(Cow::Owned(read_file(file)?)),
    }
}

/// `pub fn <name>(<receiver>: …)`, which `sofie gen` reads without compiling; with its receiver's name.
pub(crate) fn builder_function<'items>(
    file: &Path,
    items: &'items [Item],
    name: &str,
    signature: &str,
) -> Result<(&'items ItemFn, String), ModelError> {
    let function = items
        .iter()
        .find_map(|item| match item {
            Item::Fn(function) if function.sig.ident == name && is_public(&function.vis) => {
                Some(function)
            }
            _ => None,
        })
        .ok_or_else(|| {
            ModelError::in_file(
                file,
                format!(
                    "no {signature} found; add it, since `sofie gen` wires this function into the app"
                ),
            )
        })?;
    let mut inputs = function.sig.inputs.iter();
    if let (Some(FnArg::Typed(argument)), None) = (inputs.next(), inputs.next())
        && let Pat::Ident(receiver) = argument.pat.as_ref()
    {
        return Ok((function, receiver.ident.to_string()));
    }
    Err(ModelError::new(
        file,
        line_of(&function.sig),
        format!("the {name} function takes exactly one argument: write {signature}"),
    ))
}

/// As written, since a modifier's argument is copied into generated code.
pub(crate) fn single_argument(call: &ExprMethodCall) -> Option<String> {
    match (call.args.len(), call.args.first()) {
        (1, Some(argument)) => Some(
            argument
                .span()
                .source_text()
                .unwrap_or_else(|| "…".to_owned()),
        ),
        _ => None,
    }
}

pub(crate) fn is_public(visibility: &Visibility) -> bool {
    matches!(visibility, Visibility::Public(_))
}

pub(crate) fn read_file(file: &Path) -> Result<syn::File, ModelError> {
    let text = fs::read_to_string(file)
        .map_err(|error| ModelError::in_file(file, format!("cannot read the file: {error}")))?;
    parse_tokens(file, tokenize(file, &text)?)
}

/// An editor's byte order mark is not Rust, but the compiler skips it.
pub fn tokenize(file: &Path, text: &str) -> Result<TokenStream, ModelError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.parse().map_err(|error: proc_macro2::LexError| {
        ModelError::new(
            file,
            error.span().start().line,
            format!("invalid Rust: {error}"),
        )
    })
}

pub fn parse_tokens(file: &Path, tokens: TokenStream) -> Result<syn::File, ModelError> {
    syn::parse2(tokens).map_err(|error| {
        ModelError::new(
            file,
            line_of(&error.span()),
            format!("invalid Rust: {error}"),
        )
    })
}

pub(crate) fn line_of(node: &impl Spanned) -> usize {
    node.span().start().line
}

/// By its last segment, so `#[sofie::contract]` and `#[contract]` are the same attribute.
pub(crate) fn is_attribute(attribute: &Attribute, name: &str) -> bool {
    path_ends_with(attribute.path(), &[name])
}

pub(crate) fn has_attribute(attributes: &[Attribute], name: &str) -> bool {
    attributes
        .iter()
        .any(|attribute| is_attribute(attribute, name))
}

pub(crate) fn path_ends_with(path: &syn::Path, names: &[&str]) -> bool {
    path.segments.len() >= names.len()
        && path
            .segments
            .iter()
            .rev()
            .zip(names.iter().rev())
            .all(|(segment, name)| segment.ident == name)
}

pub(crate) fn segments(path: &syn::Path) -> Vec<String> {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect()
}

pub(crate) fn last_name(path: &syn::Path) -> Option<String> {
    path.segments
        .last()
        .map(|segment| segment.ident.to_string())
}

pub(crate) fn type_last_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(path) => last_name(&path.path),
        _ => None,
    }
}

pub(crate) fn first_type_argument(segment: &PathSegment) -> Option<&Type> {
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    match arguments.args.first() {
        Some(GenericArgument::Type(argument)) => Some(argument),
        _ => None,
    }
}

/// A `/** */` doc is one attribute of several lines, each decorated with a `*`.
pub(crate) fn doc_lines(attributes: &[Attribute]) -> Vec<String> {
    attributes
        .iter()
        .filter_map(|attribute| match &attribute.meta {
            syn::Meta::NameValue(pair) if pair.path.is_ident("doc") => string_literal(&pair.value),
            _ => None,
        })
        .flat_map(|text| {
            if !text.contains('\n') {
                return vec![text.trim().to_owned()];
            }
            let lines: Vec<&str> = text
                .lines()
                .map(|line| {
                    let line = line.trim();
                    line.strip_prefix('*').unwrap_or(line).trim()
                })
                .collect();
            let first = lines.iter().position(|line| !line.is_empty());
            let last = lines.iter().rposition(|line| !line.is_empty());
            match (first, last) {
                (Some(first), Some(last)) => lines[first..=last]
                    .iter()
                    .map(|line| (*line).to_owned())
                    .collect(),
                _ => Vec::new(),
            }
        })
        .collect()
}

pub(crate) fn doc_line(attributes: &[Attribute]) -> Option<String> {
    doc_lines(attributes)
        .into_iter()
        .next()
        .filter(|line| !line.is_empty())
}

/// `vec![x; n]` is no comma list; a body that is neither (`info!(key = %value)`) is skipped.
pub(crate) fn macro_expressions(call: &Macro) -> Vec<Expr> {
    if let Ok(list) = call.parse_body_with(Punctuated::<Expr, Token![,]>::parse_terminated) {
        return list.into_iter().collect();
    }
    call.parse_body_with(Punctuated::<Expr, Token![;]>::parse_terminated)
        .map(|repeated| repeated.into_iter().collect())
        .unwrap_or_default()
}

pub(crate) fn string_literal(expression: &Expr) -> Option<String> {
    let Expr::Lit(literal) = expression else {
        return None;
    };
    match &literal.lit {
        Lit::Str(text) => Some(text.value()),
        _ => None,
    }
}

pub(crate) fn public_uses(items: &[Item]) -> impl Iterator<Item = UsePath> {
    items
        .iter()
        .filter_map(|item| match item {
            Item::Use(item_use) if is_public(&item_use.vis) => Some(use_paths(&item_use.tree)),
            _ => None,
        })
        .flatten()
}

#[must_use]
pub fn use_paths(tree: &UseTree) -> Vec<UsePath> {
    let mut found = Vec::new();
    collect_use_paths(tree, &mut Vec::new(), &mut found);
    found
}

fn collect_use_paths(tree: &UseTree, prefix: &mut Vec<String>, found: &mut Vec<UsePath>) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            collect_use_paths(&path.tree, prefix, found);
            prefix.pop();
        }
        UseTree::Name(name) => found.push(named(prefix, &name.ident, &name.ident)),
        UseTree::Rename(rename) => found.push(named(prefix, &rename.ident, &rename.rename)),
        UseTree::Glob(glob) => found.push(UsePath {
            segments: prefix.clone(),
            binding: None,
            line: line_of(glob),
        }),
        UseTree::Group(group) => {
            for nested in &group.items {
                collect_use_paths(nested, prefix, found);
            }
        }
    }
}

fn named(prefix: &[String], target: &syn::Ident, binding: &syn::Ident) -> UsePath {
    let mut segments = prefix.to_vec();
    if target != "self" {
        segments.push(target.to_string());
    }
    let binding = if binding == "self" {
        prefix.last().cloned()
    } else {
        Some(binding.to_string())
    };
    UsePath {
        segments,
        binding,
        line: line_of(target),
    }
}

#[cfg(test)]
mod tests {
    use super::{UsePath, doc_lines, use_paths};

    #[test]
    fn a_block_doc_gives_one_line_per_line_without_its_stars() {
        let item: syn::ItemStruct = syn::parse_str(
            "/// First.\n/**\n * Second,\n * third.\n */\n/** Fourth. */\nstruct Config;",
        )
        .unwrap();

        assert_eq!(
            doc_lines(&item.attrs),
            ["First.", "Second,", "third.", "Fourth."]
        );
    }

    fn paths(item: &str) -> Vec<(String, Option<String>)> {
        let item: syn::ItemUse = syn::parse_str(item).unwrap();
        use_paths(&item.tree)
            .into_iter()
            .map(
                |UsePath {
                     segments, binding, ..
                 }| (segments.join("::"), binding),
            )
            .collect()
    }

    #[test]
    fn expands_groups_renames_self_and_globs() {
        let found = paths("use a::{b, c as d, e::{self}, f::{self as g}, h::*};");

        let expected = [
            ("a::b", Some("b")),
            ("a::c", Some("d")),
            ("a::e", Some("e")),
            ("a::f", Some("g")),
            ("a::h", None),
        ]
        .map(|(path, binding)| (path.to_owned(), binding.map(str::to_owned)));
        assert_eq!(found, expected);
    }
}
