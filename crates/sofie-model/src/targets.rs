//! The files cargo compiles into a crate's test, example and bench targets:
//! each target's root and the modules it declares, so a file no target
//! declares, such as an intentionally invalid fixture, is not read.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use syn::Item;

use crate::load::{files_with_extension, subdirectories};
use crate::sources::{file_name, normalize, parent, path_attribute};
use crate::syntax::read_file;
use crate::{ModelError, Source};

const TARGET_DIRECTORIES: [&str; 3] = ["tests", "examples", "benches"];

pub(crate) fn target_sources(crate_dir: &Path) -> Result<Vec<Source>, ModelError> {
    let mut sources = Vec::new();
    for directory in TARGET_DIRECTORIES {
        let mut tree = TargetTree::default();
        for root in target_roots(&crate_dir.join(directory))? {
            tree.visit(&root, &parent(&root))?;
        }
        sources.extend(
            tree.files
                .iter()
                .map(|(file, syntax)| Source::of_syntax(file, syntax)),
        );
    }
    Ok(sources)
}

/// `<directory>/*.rs` and `<directory>/*/main.rs`, as cargo discovers targets.
fn target_roots(directory: &Path) -> Result<Vec<PathBuf>, ModelError> {
    let mut roots = files_with_extension(directory, "rs")?;
    roots.extend(
        subdirectories(directory)?
            .into_iter()
            .map(|folder| folder.join("main.rs"))
            .filter(|main| main.is_file()),
    );
    Ok(roots)
}

#[derive(Default)]
struct TargetTree {
    visited: BTreeSet<PathBuf>,
    files: BTreeMap<PathBuf, syn::File>,
}

impl TargetTree {
    fn visit(&mut self, file: &Path, module_dir: &Path) -> Result<(), ModelError> {
        if !self.visited.insert(file.to_path_buf()) {
            return Ok(());
        }
        let syntax = read_file(file)?;
        self.visit_items(&syntax.items, file, module_dir, false)?;
        self.files.insert(file.to_path_buf(), syntax);
        Ok(())
    }

    fn visit_items(
        &mut self,
        items: &[Item],
        file: &Path,
        module_dir: &Path,
        inline: bool,
    ) -> Result<(), ModelError> {
        for item in items {
            let Item::Mod(module) = item else {
                continue;
            };
            let name = file_name(module);
            let path = path_attribute(file, &module.attrs)?;
            if let Some((_, inner)) = &module.content {
                let inner_dir = module_dir.join(path.unwrap_or(name));
                self.visit_items(inner, file, &inner_dir, true)?;
                continue;
            }
            if let Some(path) = path {
                let base = if inline {
                    module_dir.to_path_buf()
                } else {
                    parent(file)
                };
                let target = normalize(&base.join(path));
                if target.is_file() {
                    self.visit(&target, &parent(&target))?;
                }
                continue;
            }
            let flat = module_dir.join(format!("{name}.rs"));
            let nested = module_dir.join(&name).join("mod.rs");
            if flat.is_file() {
                self.visit(&flat, &module_dir.join(&name))?;
            } else if nested.is_file() {
                self.visit(&nested, &parent(&nested))?;
            }
        }
        Ok(())
    }
}
