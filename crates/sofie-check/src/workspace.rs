use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::TokenStream;
use sofie_model::syntax::{parse_tokens, tokenize};
use sofie_model::{ModelError, rust_files, subdirectories};

use crate::lines::{LineKind, classify_lines};

/// The benchmark apps follow other frameworks' conventions; `node_modules`
/// can be large, and the walk runs in every build script.
const SKIPPED_DIRECTORIES: [&str; 3] = ["target", "bench", "node_modules"];

const SOURCE_DIRECTORIES: [&str; 2] = ["src", "tests"];

pub struct CrateSource {
    pub name: String,
    pub files: Vec<SourceFile>,
}

pub struct SourceFile {
    pub path: PathBuf,
    pub lines: Vec<LineKind>,
    pub tokens: TokenStream,
    pub syntax: syn::File,
}

impl SourceFile {
    pub fn parse(path: PathBuf, text: &str) -> Result<Self, ModelError> {
        let tokens = tokenize(&path, text)?;
        let lines = classify_lines(text, &tokens);
        let syntax = parse_tokens(&path, tokens.clone())?;
        Ok(Self {
            path,
            lines,
            tokens,
            syntax,
        })
    }
}

pub fn load_workspace_crates(workspace_root: &Path) -> Result<Vec<CrateSource>, ModelError> {
    let mut crate_directories = Vec::new();
    find_crates(workspace_root, &mut crate_directories)?;
    crate_directories
        .iter()
        .map(|directory| load_crate(workspace_root, directory))
        .collect()
}

fn find_crates(directory: &Path, found: &mut Vec<PathBuf>) -> Result<(), ModelError> {
    if is_package(directory)? {
        found.push(directory.to_path_buf());
    }
    for subdirectory in subdirectories(directory)? {
        if !is_skipped(&subdirectory) {
            find_crates(&subdirectory, found)?;
        }
    }
    Ok(())
}

fn is_skipped(directory: &Path) -> bool {
    directory
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.') || SKIPPED_DIRECTORIES.contains(&name))
}

fn is_package(directory: &Path) -> Result<bool, ModelError> {
    let manifest = directory.join("Cargo.toml");
    if !manifest.is_file() {
        return Ok(false);
    }
    Ok(read_text(&manifest)?
        .lines()
        .any(|line| line.trim() == "[package]"))
}

fn load_crate(workspace_root: &Path, crate_directory: &Path) -> Result<CrateSource, ModelError> {
    let mut files = Vec::new();
    for source_directory in SOURCE_DIRECTORIES {
        for path in rust_files(&crate_directory.join(source_directory))? {
            files.push(load_file(workspace_root, &path)?);
        }
    }
    Ok(CrateSource {
        name: relative(workspace_root, crate_directory)
            .display()
            .to_string(),
        files,
    })
}

fn load_file(workspace_root: &Path, path: &Path) -> Result<SourceFile, ModelError> {
    SourceFile::parse(relative(workspace_root, path), &read_text(path)?)
}

fn relative(workspace_root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(workspace_root)
        .map_or_else(|_| path.to_path_buf(), Path::to_path_buf)
}

pub(crate) fn read_text(path: &Path) -> Result<String, ModelError> {
    fs::read_to_string(path)
        .map_err(|error| ModelError::in_file(path, format!("cannot read the file: {error}")))
}

#[cfg(test)]
mod tests {
    use sofie_model::testing::TemporaryDirectory;

    use super::load_workspace_crates;

    #[test]
    fn finds_every_package_at_any_depth_but_build_output_benchmarks_node_modules_and_hidden_ones() {
        let workspace = TemporaryDirectory::new("sofie-check-workspace");
        let package = "[package]\nname = \"a\"\n";
        workspace.write("Cargo.toml", "[workspace]\nmembers = [\"*\"]\n");
        for directory in [
            "shop/app",
            "shop/modules/order",
            "shop/modules/order/contract",
            "target/x",
            "bench/y",
            "shop/web/node_modules/w",
            ".z",
        ] {
            workspace.write(&format!("{directory}/Cargo.toml"), package);
            workspace.write(&format!("{directory}/src/lib.rs"), "fn f() {}\n");
        }
        workspace.write("notes/Cargo.toml", "[workspace]\n");

        let crates = load_workspace_crates(workspace.root()).unwrap();

        let names: Vec<&str> = crates.iter().map(|krate| krate.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "shop/app",
                "shop/modules/order",
                "shop/modules/order/contract"
            ]
        );
        assert_eq!(crates[1].files.len(), 1);
    }
}
