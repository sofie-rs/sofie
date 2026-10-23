use std::path::{Path, PathBuf};

use syn::ext::IdentExt;
use syn::{Item, ItemMod};

use crate::ModelError;
use crate::syntax::read_file;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleDeclaration {
    Declared,
    Missing {
        file: PathBuf,
        /// 1-based line and 0-based column of the inline module's closing brace; end of file when `None`.
        before: Option<(usize, usize)>,
        missing: Vec<String>,
    },
}

pub fn module_declaration(root: &Path, path: &[&str]) -> Result<ModuleDeclaration, ModelError> {
    let mut file = root.to_path_buf();
    let mut module_dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut remaining = path;
    loop {
        let syntax = read_file(&file)?;
        match walk(&syntax.items, remaining, &module_dir) {
            Step::Declared => return Ok(ModuleDeclaration::Declared),
            Step::Missing { before, rest } => {
                return Ok(ModuleDeclaration::Missing {
                    file,
                    before,
                    missing: rest.iter().map(|level| (*level).to_owned()).collect(),
                });
            }
            Step::InFile {
                file: child,
                directory,
                rest,
            } => {
                file = child;
                module_dir = directory;
                remaining = rest;
            }
        }
    }
}

enum Step<'p> {
    Declared,
    Missing {
        before: Option<(usize, usize)>,
        rest: &'p [&'p str],
    },
    InFile {
        file: PathBuf,
        directory: PathBuf,
        rest: &'p [&'p str],
    },
}

fn walk<'p>(items: &[Item], path: &'p [&'p str], module_dir: &Path) -> Step<'p> {
    let mut items = items;
    let mut before = None;
    let mut module_dir = module_dir.to_path_buf();
    let mut remaining = path;
    while let Some((first, rest)) = remaining.split_first() {
        let Some(found) = find_module(items, first) else {
            return Step::Missing {
                before,
                rest: remaining,
            };
        };
        module_dir = module_dir.join(first);
        if rest.is_empty() {
            return Step::Declared;
        }
        let Some((brace, inner)) = &found.content else {
            let flat = module_dir.with_extension("rs");
            let file = if flat.is_file() {
                flat
            } else {
                module_dir.join("mod.rs")
            };
            return Step::InFile {
                file,
                directory: module_dir,
                rest,
            };
        };
        let close = brace.span.close().start();
        before = Some((close.line, close.column));
        items = inner;
        remaining = rest;
    }
    Step::Declared
}

fn find_module<'i>(items: &'i [Item], name: &str) -> Option<&'i ItemMod> {
    items.iter().find_map(|item| match item {
        Item::Mod(module) if module.ident.unraw() == name => Some(module),
        _ => None,
    })
}
