use std::collections::{BTreeMap, BTreeSet};

use syn::Item;

use crate::syntax::use_paths;

/// The framework's prelude brings in no app's event, so it never makes an event's glob ambiguous.
const FRAMEWORK_PRELUDE: [&str; 2] = ["sofie", "prelude"];

pub(crate) struct HandlerScope {
    module_path: Vec<String>,
    imports: BTreeMap<String, Vec<String>>,
    globs: Vec<Vec<String>>,
    other_crate_globs: Vec<Vec<String>>,
    declared: BTreeSet<String>,
}

impl HandlerScope {
    pub(crate) fn new(module_path: Vec<String>, items: &[Item]) -> Self {
        let mut imports = BTreeMap::new();
        let mut globs = Vec::new();
        let mut other_crate_globs = Vec::new();
        let used = items.iter().filter_map(|item| match item {
            Item::Use(item_use) => Some(use_paths(&item_use.tree)),
            _ => None,
        });
        for used in used.flatten() {
            match used.binding {
                Some(binding) => {
                    imports.insert(binding, used.segments);
                }
                None if is_in_crate(&used.segments) => globs.push(used.segments),
                None if used.segments != FRAMEWORK_PRELUDE => other_crate_globs.push(used.segments),
                None => {}
            }
        }
        Self {
            module_path,
            imports,
            globs,
            other_crate_globs,
            declared: items.iter().filter_map(declared_name).collect(),
        }
    }

    pub(crate) fn module_path(&self) -> &[String] {
        &self.module_path
    }

    /// A path not through `crate`, `self`, `super` or an import stays as written rather than guessed.
    pub(crate) fn resolve(&self, written: &[String]) -> Result<Vec<String>, String> {
        let Some((first, rest)) = written.split_first() else {
            return Ok(Vec::new());
        };
        let Some(imported) = self.imports.get(first) else {
            // Handlers live in the crate, so only its own globs can bring one in; behind two,
            // a name's module is unknown without reading their targets.
            return match (written, self.globs.as_slice()) {
                ([name], _) if self.declared.contains(name) => {
                    Ok([&self.module_path[..], std::slice::from_ref(name)].concat())
                }
                ([name], [glob]) => Ok([self.anchored(glob)?, vec![name.clone()]].concat()),
                _ => self.anchored(written),
            };
        };
        let mut resolved = self.anchored(imported)?;
        resolved.extend_from_slice(rest);
        Ok(resolved)
    }

    pub(crate) fn within(&self, written: &[String]) -> Vec<String> {
        match written {
            [name] => [&self.module_path[..], std::slice::from_ref(name)].concat(),
            path => self.resolve(path).unwrap_or_else(|_| path.to_vec()),
        }
    }

    pub(crate) fn callee(&self, written: &[String]) -> Vec<String> {
        match written {
            [name] if !self.imports.contains_key(name) => self.within(written),
            path => self.resolve(path).unwrap_or_else(|_| path.to_vec()),
        }
    }

    /// `crate::…` for an item of this crate, else the other crate's path, so the wiring can name it.
    pub(crate) fn qualified(&self, written: &[String]) -> Result<Vec<String>, String> {
        let Some((first, rest)) = written.split_first() else {
            return Ok(Vec::new());
        };
        let (path, rest) = match (self.imports.get(first), written) {
            (Some(imported), _) => (imported.clone(), rest),
            (None, [name]) => (self.unimported(name)?, &[][..]),
            (None, _) => (written.to_vec(), &[][..]),
        };
        if !is_in_crate(&path) {
            return Ok([path.as_slice(), rest].concat());
        }
        let anchored = self.anchored(&path)?;
        Ok([&["crate".to_owned()][..], &anchored, rest].concat())
    }

    /// An item of this module shadows every glob; behind more than one glob, which one brings
    /// the name in is unknown without reading their targets.
    fn unimported(&self, name: &str) -> Result<Vec<String>, String> {
        if self.declared.contains(name) {
            return Ok(vec!["self".to_owned(), name.to_owned()]);
        }
        let globs: Vec<&Vec<String>> = self.globs.iter().chain(&self.other_crate_globs).collect();
        match globs.as_slice() {
            [] => Ok(vec!["self".to_owned(), name.to_owned()]),
            [glob] => Ok([glob.as_slice(), &[name.to_owned()]].concat()),
            _ => {
                let written: Vec<String> = globs
                    .iter()
                    .map(|glob| format!("`{}::*`", glob.join("::")))
                    .collect();
                Err(format!(
                    "`{name}` is not imported by name, and any of {} may bring it in; import it \
                     by name, as `use …::{name};`",
                    written.join(", ")
                ))
            }
        }
    }

    fn anchored(&self, path: &[String]) -> Result<Vec<String>, String> {
        match path.first().map(String::as_str) {
            Some("crate") => Ok(path[1..].to_vec()),
            Some("self") => Ok([&self.module_path[..], &path[1..]].concat()),
            Some("super") => self.above(path),
            _ => Ok(path.to_vec()),
        }
    }

    fn above(&self, path: &[String]) -> Result<Vec<String>, String> {
        let levels = path
            .iter()
            .take_while(|segment| *segment == "super")
            .count();
        let depth = self.module_path.len().checked_sub(levels).ok_or_else(|| {
            format!(
                "`{}` goes above the crate root, since this file is module `crate::{}`; \
                 write the handler as `crate::actions::…` or `crate::queries::…`",
                path.join("::"),
                self.module_path.join("::")
            )
        })?;
        Ok([&self.module_path[..depth], &path[levels..]].concat())
    }
}

fn is_in_crate(path: &[String]) -> bool {
    path.first()
        .is_some_and(|first| matches!(first.as_str(), "crate" | "self" | "super"))
}

fn declared_name(item: &Item) -> Option<String> {
    match item {
        Item::Fn(item) => Some(item.sig.ident.to_string()),
        Item::Struct(item) => Some(item.ident.to_string()),
        Item::Enum(item) => Some(item.ident.to_string()),
        Item::Type(item) => Some(item.ident.to_string()),
        _ => None,
    }
}
