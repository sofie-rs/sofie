//! Names are matched without resolving paths: the dead code check can miss dead code but never calls live code dead.

use std::collections::BTreeMap;

use proc_macro2::{TokenStream, TokenTree};
use syn::ext::IdentExt;
use syn::visit::{self, Visit};
use syn::{Attribute, ImplItem, Item, ItemImpl, ItemUse, LitStr, Macro, MetaList, Type};

use crate::syntax::{is_public, line_of};

/// String literals count, since a generator, a format string or `serde(with = "…")` can name an item there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mentions {
    counts: BTreeMap<String, usize>,
}

impl Mentions {
    #[must_use]
    pub fn of(syntax: &syn::File) -> Self {
        let mut mentions = Self::default();
        mentions.visit_file(syntax);
        mentions
    }

    pub fn add(&mut self, other: &Self) {
        for (name, count) in &other.counts {
            *self.counts.entry(name.clone()).or_default() += count;
        }
    }

    #[must_use]
    pub fn count(&self, name: &str) -> usize {
        self.counts.get(name).copied().unwrap_or_default()
    }

    fn mention(&mut self, name: String) {
        *self.counts.entry(name).or_default() += 1;
    }

    fn mention_tokens(&mut self, tokens: TokenStream) {
        let mut after_pound = false;
        for tree in tokens {
            match tree {
                TokenTree::Ident(ident) => self.mention(ident.unraw().to_string()),
                TokenTree::Literal(literal) => self.mention_words(&literal.to_string()),
                TokenTree::Group(group) if !(after_pound && is_doc_tokens(&group.stream())) => {
                    self.mention_tokens(group.stream());
                }
                TokenTree::Group(_) => {}
                TokenTree::Punct(ref punct) => {
                    after_pound = punct.as_char() == '#' || (after_pound && punct.as_char() == '!');
                    continue;
                }
            }
            after_pound = false;
        }
    }

    fn mention_words(&mut self, literal: &str) {
        if !literal.contains('"') {
            return;
        }
        let words = literal
            .split(|character: char| !(character.is_alphanumeric() || character == '_'))
            .filter(|word| word.starts_with(|first: char| first.is_alphabetic() || first == '_'));
        for word in words {
            self.mention(word.to_owned());
        }
    }
}

impl<'ast> Visit<'ast> for Mentions {
    fn visit_ident(&mut self, ident: &'ast proc_macro2::Ident) {
        self.mention(ident.unraw().to_string());
    }

    fn visit_lit_str(&mut self, literal: &'ast LitStr) {
        self.mention_words(&literal.token().to_string());
    }

    fn visit_attribute(&mut self, attribute: &'ast Attribute) {
        if !attribute.path().is_ident("doc") {
            visit::visit_attribute(self, attribute);
        }
    }

    fn visit_meta_list(&mut self, list: &'ast MetaList) {
        visit::visit_meta_list(self, list);
        self.mention_tokens(list.tokens.clone());
    }

    fn visit_macro(&mut self, macro_call: &'ast Macro) {
        visit::visit_macro(self, macro_call);
        self.mention_tokens(macro_call.tokens.clone());
    }

    fn visit_item_use(&mut self, item_use: &'ast ItemUse) {
        if !is_public(&item_use.vis) {
            visit::visit_item_use(self, item_use);
        }
    }

    /// `impl Order { … }` and `impl Display for Order` do not use `Order`;
    /// generic arguments of the type and the trait still count.
    fn visit_item_impl(&mut self, implementation: &'ast ItemImpl) {
        for attribute in &implementation.attrs {
            self.visit_attribute(attribute);
        }
        self.visit_generics(&implementation.generics);
        if let Some((trait_path, _)) = &implementation.trait_ {
            self.visit_path(trait_path);
        }
        match implementation.self_ty.as_ref() {
            Type::Path(self_type) => {
                for segment in &self_type.path.segments {
                    self.visit_path_arguments(&segment.arguments);
                }
            }
            other => self.visit_type(other),
        }
        for item in &implementation.items {
            self.visit_impl_item(item);
        }
    }
}

fn is_doc_tokens(attribute: &TokenStream) -> bool {
    matches!(attribute.clone().into_iter().next(), Some(TokenTree::Ident(ident)) if ident == "doc")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicItem {
    pub name: String,
    pub line: usize,
}

#[must_use]
pub fn public_items(syntax: &syn::File) -> Vec<PublicItem> {
    let mut found = Vec::new();
    collect_items(&syntax.items, &mut found);
    found
}

fn collect_items(items: &[Item], found: &mut Vec<PublicItem>) {
    for item in items {
        match item {
            Item::Mod(module) if !is_test_only(&module.attrs) => {
                if let Some((_, inner)) = &module.content {
                    collect_items(inner, found);
                }
            }
            Item::Impl(implementation) if implementation.trait_.is_none() => {
                collect_impl_items(&implementation.items, found);
            }
            Item::Fn(function)
                if function.sig.ident == "main" || is_procedural_macro(&function.attrs) => {}
            _ => found.extend(public_item(item)),
        }
    }
}

fn collect_impl_items(items: &[ImplItem], found: &mut Vec<PublicItem>) {
    for item in items {
        let (visibility, ident, attributes) = match item {
            ImplItem::Fn(function) => (&function.vis, &function.sig.ident, &function.attrs),
            ImplItem::Const(constant) => (&constant.vis, &constant.ident, &constant.attrs),
            _ => continue,
        };
        if is_public(visibility) && !is_test_only(attributes) {
            found.push(named(ident));
        }
    }
}

fn public_item(item: &Item) -> Option<PublicItem> {
    let (visibility, ident, attributes) = match item {
        Item::Const(item) => (&item.vis, &item.ident, &item.attrs),
        Item::Enum(item) => (&item.vis, &item.ident, &item.attrs),
        Item::Fn(item) => (&item.vis, &item.sig.ident, &item.attrs),
        Item::Static(item) => (&item.vis, &item.ident, &item.attrs),
        Item::Struct(item) => (&item.vis, &item.ident, &item.attrs),
        Item::Trait(item) => (&item.vis, &item.ident, &item.attrs),
        Item::TraitAlias(item) => (&item.vis, &item.ident, &item.attrs),
        Item::Type(item) => (&item.vis, &item.ident, &item.attrs),
        Item::Union(item) => (&item.vis, &item.ident, &item.attrs),
        _ => return None,
    };
    (is_public(visibility) && !is_test_only(attributes)).then(|| named(ident))
}

fn named(ident: &proc_macro2::Ident) -> PublicItem {
    PublicItem {
        name: ident.unraw().to_string(),
        line: line_of(ident),
    }
}

fn is_test_only(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("test")
            || (attribute.path().is_ident("cfg")
                && attribute
                    .meta
                    .require_list()
                    .is_ok_and(|list| list.tokens.to_string() == "test"))
    })
}

fn is_procedural_macro(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        ["proc_macro", "proc_macro_derive", "proc_macro_attribute"]
            .iter()
            .any(|name| attribute.path().is_ident(name))
    })
}

#[cfg(test)]
mod tests {
    use super::{Mentions, public_items};

    fn parse(source: &str) -> syn::File {
        syn::parse_str(source).unwrap()
    }

    #[test]
    fn counts_names_outside_docs_re_exports_and_impl_headers() {
        let file = parse(
            "pub use a::Reexported;\nuse b::Imported;\n\
             impl Display for Order<Line> { fn f() { g!(Macro, \"{Formatted} x\"); } }\n\
             /// Uses `Documented`.\n#[serde(with = \"Attributed\")]\nstruct S;\n",
        );

        let mentions = Mentions::of(&file);

        for name in [
            "Imported",
            "Display",
            "Line",
            "Macro",
            "Formatted",
            "Attributed",
        ] {
            assert_eq!(mentions.count(name), 1, "{name}");
        }
        for name in ["Documented", "Reexported", "Order"] {
            assert_eq!(mentions.count(name), 0, "{name}");
        }
    }

    #[test]
    fn lists_public_items_but_trait_impls_main_macros_and_tests() {
        let file = parse(
            "pub struct Kept;\nstruct Private;\nimpl Kept { pub fn method() {} fn private() {} }\n\
             impl Clone for Kept { fn clone(&self) -> Self { Kept } }\npub fn main() {}\n\
             #[proc_macro_derive(X)]\npub fn derive(item: T) -> T { item }\n\
             #[cfg(test)]\nmod tests { pub fn helper() {} }\nmod inner { pub const LIMIT: u8 = 1; }\n",
        );

        let names: Vec<String> = public_items(&file)
            .into_iter()
            .map(|item| item.name)
            .collect();

        assert_eq!(names, ["Kept", "method", "LIMIT"]);
    }
}
