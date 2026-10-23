use syn::visit::{self, Visit};
use syn::{Item, UseTree};

use super::Edit;

const MOVED: [&str; 2] = ["Password", "PasswordHash"];
const FROM: [&[&str]; 2] = [&["sofie", "auth"], &["sofie_auth"]];
const TO: &str = "sofie_identity";

/// `sofie::auth::Password` and `sofie_auth::Password` become
/// `sofie_identity::Password`; a file reaching `Password` through
/// `sofie::prelude::*`, which no longer carries it, gets the import.
pub(super) fn edits(_: &str, file: &syn::File) -> Vec<Edit> {
    let mut rewriter = Rewriter::default();
    rewriter.visit_file(file);
    let mut edits = rewriter.edits;
    if let Some(prelude) = file.items.iter().find_map(prelude_end) {
        let needs = FileNeeds::of(file);
        let named: Vec<&str> = MOVED
            .into_iter()
            .filter(|moved| needs.bare.contains(moved) && !needs.imported.contains(moved))
            .collect();
        let import = match named.as_slice() {
            [] => None,
            [one] => Some(format!("\nuse {TO}::{one};")),
            several => Some(format!("\nuse {TO}::{{{}}};", several.join(", "))),
        };
        edits.extend(import.map(|text| Edit::insert(prelude, text)));
    }
    edits
}

#[derive(Default)]
struct Rewriter {
    edits: Vec<Edit>,
}

impl<'ast> Visit<'ast> for Rewriter {
    fn visit_item(&mut self, item: &'ast Item) {
        if let Item::Use(used) = item {
            self.rewrite_use(&used.tree, &mut Vec::new(), used);
            return;
        }
        visit::visit_item(self, item);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        let segments: Vec<String> = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        if let Some(moved) = old_path(&segments) {
            let last = path.segments.last().expect("a moved path has segments");
            self.edits.push(Edit {
                start: path.segments[0].ident.span().start(),
                end: last.ident.span().end(),
                text: format!("{TO}::{moved}"),
            });
        }
        visit::visit_path(self, path);
    }
}

impl Rewriter {
    /// Only a `use` naming one moved item is rewritten; one inside a group
    /// with other items is left for the compiler to point at.
    fn rewrite_use(&mut self, tree: &UseTree, prefix: &mut Vec<String>, used: &syn::ItemUse) {
        match tree {
            UseTree::Path(path) => {
                prefix.push(path.ident.to_string());
                self.rewrite_use(&path.tree, prefix, used);
                prefix.pop();
            }
            UseTree::Name(name) => {
                let mut full = prefix.clone();
                full.push(name.ident.to_string());
                if let Some(moved) = old_path(&full)
                    && !has_group(&used.tree)
                {
                    self.edits
                        .push(Edit::spanning(&used.tree, format!("{TO}::{moved}")));
                }
            }
            UseTree::Group(group) => {
                for item in &group.items {
                    self.rewrite_use(item, prefix, used);
                }
            }
            UseTree::Rename(_) | UseTree::Glob(_) => {}
        }
    }
}

/// The file's own scope only: what a nested module imports or names does not reach it.
#[derive(Default)]
struct FileNeeds {
    bare: Vec<&'static str>,
    imported: Vec<&'static str>,
}

impl FileNeeds {
    fn of(file: &syn::File) -> Self {
        let mut needs = Self::default();
        for item in &file.items {
            match item {
                Item::Use(used) => imported_names(&used.tree, &mut needs.imported),
                Item::Mod(_) => {}
                other => {
                    needs
                        .imported
                        .extend(declared_name(other).and_then(|name| moved(&name.to_string())));
                    needs.visit_item(other);
                }
            }
        }
        needs
    }
}

impl<'ast> Visit<'ast> for FileNeeds {
    fn visit_item_mod(&mut self, _: &'ast syn::ItemMod) {}

    fn visit_path(&mut self, path: &'ast syn::Path) {
        if let Some(name) = path.get_ident()
            && let Some(moved) = moved(&name.to_string())
            && !self.bare.contains(&moved)
        {
            self.bare.push(moved);
        }
        visit::visit_path(self, path);
    }
}

fn imported_names(tree: &UseTree, imported: &mut Vec<&'static str>) {
    match tree {
        UseTree::Path(path) => imported_names(&path.tree, imported),
        UseTree::Name(name) => imported.extend(moved(&name.ident.to_string())),
        UseTree::Group(group) => {
            for item in &group.items {
                imported_names(item, imported);
            }
        }
        UseTree::Rename(_) | UseTree::Glob(_) => {}
    }
}

/// Only the file's own: an import after a nested module's prelude would not reach the file.
fn prelude_end(item: &Item) -> Option<proc_macro2::LineColumn> {
    let Item::Use(used) = item else {
        return None;
    };
    let UseTree::Path(sofie) = &used.tree else {
        return None;
    };
    let globs_prelude = sofie.ident == "sofie"
        && match &*sofie.tree {
            UseTree::Group(group) => group.items.iter().any(is_prelude_glob),
            tree => is_prelude_glob(tree),
        };
    globs_prelude.then(|| syn::spanned::Spanned::span(used).end())
}

fn is_prelude_glob(tree: &UseTree) -> bool {
    matches!(tree, UseTree::Path(prelude)
        if prelude.ident == "prelude" && matches!(*prelude.tree, UseTree::Glob(_)))
}

fn declared_name(item: &Item) -> Option<&syn::Ident> {
    match item {
        Item::Struct(item) => Some(&item.ident),
        Item::Enum(item) => Some(&item.ident),
        Item::Type(item) => Some(&item.ident),
        Item::Trait(item) => Some(&item.ident),
        Item::Union(item) => Some(&item.ident),
        _ => None,
    }
}

fn has_group(tree: &UseTree) -> bool {
    match tree {
        UseTree::Path(path) => has_group(&path.tree),
        UseTree::Group(_) => true,
        UseTree::Name(_) | UseTree::Rename(_) | UseTree::Glob(_) => false,
    }
}

fn moved(name: &str) -> Option<&'static str> {
    MOVED.into_iter().find(|moved| *moved == name)
}

fn old_path(segments: &[String]) -> Option<&'static str> {
    let (last, prefix) = segments.split_last()?;
    let from_old = FROM.iter().any(|from| {
        from.len() == prefix.len() && from.iter().zip(prefix).all(|(old, given)| old == given)
    });
    from_old.then(|| moved(last)).flatten()
}

#[cfg(test)]
mod tests {
    use super::super::upgrade_text;

    #[test]
    fn rewrites_old_paths_and_single_imports() {
        let text = "use sofie::auth::PasswordHash;\n\nfn hash(password: &sofie::auth::Password) -> PasswordHash {\n    \
                    password.hash()\n}\n";

        let (upgraded, _) = upgrade_text(text).unwrap();

        assert_eq!(
            upgraded,
            "use sofie_identity::PasswordHash;\n\nfn hash(password: &sofie_identity::Password) -> PasswordHash {\n    \
             password.hash()\n}\n"
        );
    }

    #[test]
    fn imports_what_the_prelude_no_longer_carries() {
        let text = "use sofie::prelude::*;\n\n#[derive(Input)]\npub struct SignUp {\n    pub password: Password,\n}\n";

        let (upgraded, changes) = upgrade_text(text).unwrap();

        assert_eq!(
            upgraded,
            "use sofie::prelude::*;\nuse sofie_identity::Password;\n\n#[derive(Input)]\npub struct SignUp {\n    \
             pub password: Password,\n}\n"
        );
        assert_eq!(changes.len(), 1);
        assert_eq!(upgrade_text(&upgraded).unwrap().0, upgraded);
    }

    #[test]
    fn imports_after_the_file_s_own_prelude_not_a_nested_module_s() {
        let text = "use sofie::prelude::*;\n\npub struct SignUp {\n    pub password: Password,\n}\n\n\
                    mod tests {\n    use sofie::prelude::*;\n}\n";

        let (upgraded, _) = upgrade_text(text).unwrap();

        assert!(
            upgraded.starts_with("use sofie::prelude::*;\nuse sofie_identity::Password;\n\n"),
            "{upgraded}"
        );
        assert!(upgraded.ends_with("mod tests {\n    use sofie::prelude::*;\n}\n"));
    }

    #[test]
    fn imports_after_a_prelude_glob_inside_a_group() {
        let text = "use sofie::{http::Router, prelude::*};\n\npub struct SignUp {\n    pub password: Password,\n}\n";

        let (upgraded, _) = upgrade_text(text).unwrap();

        assert!(
            upgraded.starts_with(
                "use sofie::{http::Router, prelude::*};\nuse sofie_identity::Password;\n"
            ),
            "{upgraded}"
        );
    }

    #[test]
    fn a_nested_module_s_import_does_not_hide_the_file_s_need() {
        let text = "use sofie::prelude::*;\n\npub struct SignUp {\n    pub password: Password,\n}\n\n\
                    mod tests {\n    use sofie_identity::Password;\n}\n";

        let (upgraded, _) = upgrade_text(text).unwrap();

        assert!(
            upgraded.starts_with("use sofie::prelude::*;\nuse sofie_identity::Password;\n\n"),
            "{upgraded}"
        );
    }

    #[test]
    fn leaves_a_local_password_alone() {
        let text = "use sofie::prelude::*;\n\npub struct Password(String);\n\n#[derive(Input)]\npub struct SignUp {\n    \
                    pub password: Password,\n}\n";

        let (upgraded, changes) = upgrade_text(text).unwrap();

        assert_eq!(upgraded, text);
        assert_eq!(changes.len(), 0);
    }
}
