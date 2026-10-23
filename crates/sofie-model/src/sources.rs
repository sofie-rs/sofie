use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use syn::ext::IdentExt;
use syn::{Attribute, Item, ItemMod, Meta, Visibility};

use crate::handlers::HandlerScope;
use crate::messages::{Found, collect_item};
use crate::syntax::{is_public, line_of, read_file, string_literal};
use crate::{ModelError, PrivateItem, Source};

const CRATE_ROOTS: [&str; 2] = ["src/lib.rs", "src/main.rs"];

pub(crate) struct Sources {
    pub files: Vec<Source>,
    pub found: Found,
    pub syntax: BTreeMap<PathBuf, syn::File>,
    pub module_paths: BTreeMap<PathBuf, Vec<String>>,
}

/// Whether a wired item may sit under a `cfg`: the wiring of an app and its modules names
/// every one in every build, while a library's features decide which of its items exist.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConditionalWiring {
    Refused,
    Allowed,
}

/// A declared module without a file is skipped: `sofie gen` must load an app whose `mod r#gen;` it has not written yet.
pub(crate) fn module_tree(
    crate_dir: &Path,
    conditional_wiring: ConditionalWiring,
) -> Result<Sources, ModelError> {
    let roots: Vec<PathBuf> = CRATE_ROOTS
        .iter()
        .map(|root| crate_dir.join(root))
        .filter(|root| root.is_file())
        .collect();
    if roots.is_empty() {
        return Err(ModelError::in_file(
            &crate_dir.join("Cargo.toml"),
            "the crate has neither src/lib.rs nor src/main.rs; create one of them",
        ));
    }
    let mut walk = Walk {
        conditional_wiring,
        visited: BTreeSet::new(),
        syntax: BTreeMap::new(),
        module_paths: BTreeMap::new(),
        found: Found::default(),
    };
    for root in roots {
        walk.visit_file(&root, Vec::new(), None)?;
    }
    let files = walk
        .syntax
        .iter()
        .map(|(file, syntax)| Source::of_syntax(file, syntax))
        .collect();
    Ok(Sources {
        files,
        found: walk.found,
        syntax: walk.syntax,
        module_paths: walk.module_paths,
    })
}

struct Walk {
    conditional_wiring: ConditionalWiring,
    visited: BTreeSet<PathBuf>,
    syntax: BTreeMap<PathBuf, syn::File>,
    module_paths: BTreeMap<PathBuf, Vec<String>>,
    found: Found,
}

struct Scope<'a> {
    file: &'a Path,
    file_dir: PathBuf,
    module_dir: PathBuf,
    inline: bool,
    module_path: Vec<String>,
    /// The nearest `cfg` other than `test` over this scope.
    condition: Option<CfgAt>,
}

impl Scope<'_> {
    fn inline_child(&self, module: &ItemMod, condition: Option<CfgAt>) -> Result<Self, ModelError> {
        let name = path_attribute(self.file, &module.attrs)?.unwrap_or_else(|| file_name(module));
        Ok(Scope {
            file: self.file,
            file_dir: self.file_dir.clone(),
            module_dir: self.module_dir.join(name),
            inline: true,
            module_path: self.child_path(module),
            condition,
        })
    }

    fn child_path(&self, module: &ItemMod) -> Vec<String> {
        let mut path = self.module_path.clone();
        // With `r#`, as the wiring writes a keyword's module.
        path.push(module.ident.to_string());
        path
    }
}

impl Walk {
    fn visit_file(
        &mut self,
        file: &Path,
        module_path: Vec<String>,
        condition: Option<CfgAt>,
    ) -> Result<(), ModelError> {
        self.visit_file_with_module_dir(file, parent(file), module_path, condition)
    }

    fn visit_file_with_module_dir(
        &mut self,
        file: &Path,
        module_dir: PathBuf,
        module_path: Vec<String>,
        condition: Option<CfgAt>,
    ) -> Result<(), ModelError> {
        if !self.visited.insert(file.to_path_buf()) {
            return Ok(());
        }
        let syntax = read_file(file)?;
        let scope = Scope {
            file,
            file_dir: parent(file),
            module_dir,
            inline: false,
            module_path,
            condition,
        };
        self.visit_items(&syntax.items, &scope)?;
        self.syntax.insert(file.to_path_buf(), syntax);
        self.module_paths
            .insert(file.to_path_buf(), scope.module_path);
        Ok(())
    }

    fn visit_items(&mut self, items: &[Item], scope: &Scope) -> Result<(), ModelError> {
        let imports = HandlerScope::new(scope.module_path.clone(), items);
        for item in items {
            let condition = match condition(item) {
                Condition::Test => continue,
                Condition::Other(line) => Some(CfgAt {
                    file: scope.file.to_path_buf(),
                    line,
                }),
                Condition::None => scope.condition.clone(),
            };
            let Item::Mod(module) = item else {
                self.collect(item, scope.file, &imports, condition)?;
                continue;
            };
            if !is_public(&module.vis) {
                self.found.private_items.push(PrivateItem {
                    path: scope.child_path(module),
                    file: scope.file.to_path_buf(),
                    line: line_of(&module.ident),
                    crate_visible: is_crate_visible(&module.vis),
                });
            }
            match &module.content {
                Some((_, inner)) => {
                    self.visit_items(inner, &scope.inline_child(module, condition)?)?;
                }
                None => self.visit_declared(module, scope, condition)?,
            }
        }
        Ok(())
    }

    /// The wiring names a wired item in every build, so it cannot exist only in some.
    fn collect(
        &mut self,
        item: &Item,
        file: &Path,
        imports: &HandlerScope,
        condition: Option<CfgAt>,
    ) -> Result<(), ModelError> {
        let wired_before = self.found.wired_count();
        collect_item(item, file, imports, &mut self.found)?;
        let wired = self.found.wired_count() > wired_before;
        if wired
            && let Item::Fn(function) = item
            && !is_public(&function.vis)
        {
            let mut path = imports.module_path().to_vec();
            path.push(function.sig.ident.to_string());
            self.found.private_items.push(PrivateItem {
                path,
                file: file.to_path_buf(),
                line: line_of(&function.sig.ident),
                crate_visible: is_crate_visible(&function.vis),
            });
        }
        match condition {
            Some(cfg) if wired && self.conditional_wiring == ConditionalWiring::Refused => {
                Err(ModelError::new(
                    file,
                    line_of(item),
                    format!(
                        "this wired item is compiled only under the `cfg` at {}:{}, but the \
                         wiring names it in every build; remove the `cfg`, or `#[cfg(test)]` \
                         to keep it out of the wiring",
                        cfg.file.display(),
                        cfg.line
                    ),
                ))
            }
            _ => Ok(()),
        }
    }

    fn visit_declared(
        &mut self,
        module: &ItemMod,
        scope: &Scope,
        condition: Option<CfgAt>,
    ) -> Result<(), ModelError> {
        let module_path = scope.child_path(module);
        if let Some(path) = path_attribute(scope.file, &module.attrs)? {
            let base = if scope.inline {
                &scope.module_dir
            } else {
                &scope.file_dir
            };
            let target = normalize(&base.join(path));
            return if target.is_file() {
                self.visit_file(&target, module_path, condition)
            } else {
                Ok(())
            };
        }
        let name = file_name(module);
        let flat = scope.module_dir.join(format!("{name}.rs"));
        let nested = scope.module_dir.join(&name).join("mod.rs");
        match (flat.is_file(), nested.is_file()) {
            (true, false) => self.visit_file_with_module_dir(
                &flat,
                scope.module_dir.join(&name),
                module_path,
                condition,
            ),
            (false, true) => self.visit_file(&nested, module_path, condition),
            (true, true) => Err(ModelError::new(
                scope.file,
                line_of(module),
                format!(
                    "both {} and {} exist for module `{name}`; delete one",
                    flat.display(),
                    nested.display()
                ),
            )),
            (false, false) => Ok(()),
        }
    }
}

#[derive(Clone)]
struct CfgAt {
    file: PathBuf,
    line: usize,
}

enum Condition {
    None,
    Test,
    Other(usize),
}

/// `cfg(test)` code is never in the app, so the wiring skips it.
fn condition(item: &Item) -> Condition {
    let Some(cfg) = attributes(item)
        .iter()
        .find(|attribute| attribute.path().is_ident("cfg"))
    else {
        return Condition::None;
    };
    let is_test = cfg
        .parse_args::<Meta>()
        .is_ok_and(|predicate| predicate.path().is_ident("test"));
    if is_test {
        Condition::Test
    } else {
        Condition::Other(line_of(cfg))
    }
}

fn attributes(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        _ => &[],
    }
}

pub(crate) fn path_attribute(
    file: &Path,
    attrs: &[Attribute],
) -> Result<Option<String>, ModelError> {
    let Some(attr) = attrs.iter().find(|attr| attr.path().is_ident("path")) else {
        return Ok(None);
    };
    if let Meta::NameValue(pair) = &attr.meta
        && let Some(path) = string_literal(&pair.value)
    {
        return Ok(Some(path));
    }
    Err(ModelError::new(
        file,
        line_of(attr),
        "write the module path as `#[path = \"file.rs\"]`",
    ))
}

/// `pub(crate)` or `pub(in crate)`.
fn is_crate_visible(visibility: &Visibility) -> bool {
    matches!(visibility, Visibility::Restricted(restricted) if restricted.path.is_ident("crate"))
}

pub(crate) fn file_name(module: &ItemMod) -> String {
    module.ident.unraw().to_string()
}

pub(crate) fn parent(file: &Path) -> PathBuf {
    file.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// Resolves `..` lexically, so a `#[path]` file compares equal to
/// the same file reached by name.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir if ends_in_name(&normalized) => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    normalized
}

fn ends_in_name(path: &Path) -> bool {
    matches!(path.components().next_back(), Some(Component::Normal(_)))
}
