use std::fmt;
use std::path::{Path, PathBuf};

use sofie_check::{ALLOW_FILE, AllowList};
use sofie_model::naming::{crate_name, relative_path};
use sofie_model::{Crate, KnownArea};

use super::{framework_source, refuse_existing};
use crate::gen_command::GenError;
use crate::rustfmt::format_rust;
use crate::source_edits::{
    DEPENDENCIES, InlineFeatures, Journal, add_import, declared_module, inline_features, insert_at,
    path_dependency, read, toml_section, with_dependency_line, without_toml_comment,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileChange {
    Created(PathBuf),
    Changed(PathBuf),
    /// What the generator chose that the files do not say, such as a field it left out.
    Note(String),
}

impl FileChange {
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Created(path) | Self::Changed(path) => Some(path),
            Self::Note(_) => None,
        }
    }
}

impl fmt::Display for FileChange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Created(path) => write!(formatter, "created {}", path.display()),
            Self::Changed(path) => write!(formatter, "changed {}", path.display()),
            Self::Note(note) => write!(formatter, "note: {note}"),
        }
    }
}

#[derive(Default)]
pub(super) struct Edits {
    changes: Vec<FileChange>,
    journal: Journal,
}

impl Edits {
    pub(super) fn create(&mut self, file: &Path, text: &str) -> Result<(), GenError> {
        refuse_existing(file)?;
        self.journal.write(file, text)?;
        self.changes.push(FileChange::Created(file.to_path_buf()));
        Ok(())
    }

    fn write(&mut self, file: &Path, text: &str) -> Result<(), GenError> {
        self.journal.write(file, text)?;
        self.changed(file);
        Ok(())
    }

    pub(super) fn create_rust(&mut self, file: &Path, text: &str) -> Result<(), GenError> {
        self.create(file, &format_rust(text, file).map_err(GenError::Rustfmt)?)
    }

    pub(super) fn edit_rust(
        &mut self,
        file: &Path,
        edit: impl FnOnce(&str) -> Result<String, GenError>,
    ) -> Result<(), GenError> {
        let text = read(file)?;
        let edited = format_rust(&edit(&text)?, file).map_err(GenError::Rustfmt)?;
        self.write(file, &edited)
    }

    /// Adds `item` to the end of `file` with the `imports` it needs, creating the file when missing.
    pub(super) fn append(
        &mut self,
        file: &Path,
        imports: &[(&str, &str)],
        item: &str,
    ) -> Result<(), GenError> {
        let with_item = |text: &str| format!("{}\n{item}", with_imports(text, imports));
        if file.is_file() {
            return self.edit_rust(file, |text| Ok(with_item(text)));
        }
        self.create_rust(file, &with_item(""))
    }

    /// Declares the module at `path` from `krate`'s `lib.rs`.
    pub(super) fn declare(&mut self, krate: &Crate, path: &[&str]) -> Result<(), GenError> {
        match declared_module(&krate.path.join("src").join("lib.rs"), path)? {
            Some((file, text)) => self.write(&file, &text),
            None => Ok(()),
        }
    }

    pub(super) fn depend(&mut self, dependent: &Crate, dependency: &Crate) -> Result<(), GenError> {
        if dependent.depends_on(&dependency.package) {
            return Ok(());
        }
        let path = relative_path(&dependent.path, &dependency.path).ok_or_else(|| {
            GenError::Usage(format!(
                "{} cannot name {} by a relative path",
                dependent.path.display(),
                dependency.path.display()
            ))
        })?;
        self.add_dependency(dependent, &path_dependency(&dependency.package, &path))
    }

    /// From the workspace, as in every manifest `sofie new` writes. Known by its
    /// crate name: the package is `sofie-framework`.
    pub(super) fn depend_on_framework(&mut self, dependent: &Crate) -> Result<(), GenError> {
        let present = dependent
            .dependencies
            .iter()
            .any(|dependency| dependency.crate_name == "sofie");
        if present {
            return Ok(());
        }
        self.add_dependency(dependent, "sofie.workspace = true\n")
    }

    fn add_dependency(&mut self, dependent: &Crate, line: &str) -> Result<(), GenError> {
        let manifest = dependent.path.join("Cargo.toml");
        let text = with_dependency_line(&read(&manifest)?, line);
        self.write(&manifest, &text)
    }

    pub(super) fn enable_framework_feature(
        &mut self,
        krate: &Crate,
        feature: &str,
    ) -> Result<(), GenError> {
        self.enable_feature(krate, "sofie", feature)
    }

    pub(super) fn depend_on_workspace(
        &mut self,
        krate: &Crate,
        package: &str,
    ) -> Result<(), GenError> {
        if krate.depends_on(package) {
            return Ok(());
        }
        self.declare_in_workspace(krate, package)?;
        self.add_dependency(krate, &format!("{package}.workspace = true\n"))
    }

    fn declare_in_workspace(&mut self, krate: &Crate, package: &str) -> Result<(), GenError> {
        match workspace_declaration(&krate.path, package)? {
            Some((manifest, text)) => self.write(&manifest, &text),
            None => Ok(()),
        }
    }

    /// Adds the dependency named `package` from the workspace first when `krate` lacks it.
    pub(super) fn enable_feature(
        &mut self,
        krate: &Crate,
        package: &str,
        feature: &str,
    ) -> Result<(), GenError> {
        let wanted = crate_name(package);
        let present = krate
            .dependencies
            .iter()
            .any(|dependency| dependency.crate_name == wanted);
        if !present {
            self.declare_in_workspace(krate, package)?;
            return self.add_dependency(
                krate,
                &format!("{package} = {{ workspace = true, features = [\"{feature}\"] }}\n"),
            );
        }
        let manifest = krate.path.join("Cargo.toml");
        let text = read(&manifest)?;
        let edited = with_feature_on(&text, package, feature).ok_or_else(|| {
            GenError::Usage(format!(
                "{} has no one-line `{package}` dependency to turn the `{feature}` feature on in; \
                 add `features = [\"{feature}\"]` to it",
                manifest.display()
            ))
        })?;
        if edited == text {
            return Ok(());
        }
        self.write(&manifest, &edited)
    }

    /// Lets a scaffolded item pass the dead code check until code uses it;
    /// `sofie fix` then removes the entry, which `sofie check` reports as stale.
    pub(super) fn allow_unused(
        &mut self,
        root: &Path,
        package: &str,
        name: &str,
        generator: &str,
    ) -> Result<(), GenError> {
        let file = root.join(ALLOW_FILE);
        let reason = format!(
            "scaffolded by `sofie new {generator}`; `sofie fix` removes this entry once code uses it"
        );
        let entry = AllowList::item_entry(package, name, &reason);
        if !file.is_file() {
            return self.create(&file, &entry);
        }
        self.write(&file, &(read(&file)? + &entry))
    }

    /// A generator that uses a scaffolded item removes its entry, which `sofie check` would report as stale.
    pub(super) fn stop_allowing_unused(
        &mut self,
        root: &Path,
        package: &str,
        name: &str,
    ) -> Result<(), GenError> {
        let Some(line) = AllowList::read(root)
            .map_err(GenError::Model)?
            .item_line(package, name)
        else {
            return Ok(());
        };
        let file = root.join(ALLOW_FILE);
        self.write(&file, &AllowList::without_entries(&read(&file)?, &[line]))
    }

    pub(super) fn changed(&mut self, file: &Path) {
        if self
            .changes
            .iter()
            .all(|change| change.path() != Some(file))
        {
            self.changes.push(FileChange::Changed(file.to_path_buf()));
        }
    }

    pub(super) fn note(&mut self, note: String) {
        self.changes.push(FileChange::Note(note));
    }

    /// The changes, or on a failure the error once every change is undone.
    pub(super) fn finish(self, result: Result<(), GenError>) -> Result<Vec<FileChange>, GenError> {
        self.journal.finish(result)?;
        Ok(self.changes)
    }
}

pub(super) fn with_imports(text: &str, imports: &[(&str, &str)]) -> String {
    imports.iter().fold(text.to_owned(), |text, (path, name)| {
        add_import(&text, path, name)
    })
}

/// Refuses a second `fn name(` in `file`, when it exists.
pub(super) fn refuse_function(file: &Path, name: &str) -> Result<(), GenError> {
    if file.is_file() && read(file)?.contains(&format!("fn {name}(")) {
        return Err(GenError::Usage(format!(
            "{} has a function `{name}` already; choose another name",
            file.display()
        )));
    }
    Ok(())
}

/// `src/<name>/mod.rs` when the module is a folder, else `src/<name>.rs`.
pub(super) fn module_file(krate: &Crate, name: &str) -> PathBuf {
    let source = krate.path.join("src");
    let folder = source.join(name).join("mod.rs");
    if folder.is_file() {
        return folder;
    }
    source.join(format!("{name}.rs"))
}

/// The `use` line an area's file needs for its type, and the type's name.
pub(super) fn area_type<'area>(area: &'area KnownArea<'_>) -> (String, &'area str) {
    let name = area.type_path.rsplit("::").next().unwrap_or_default();
    if area.definition.is_none() {
        return (String::new(), name);
    }
    (format!("use {};\n", area.type_path), name)
}

pub(crate) fn with_feature_on(manifest: &str, package: &str, feature: &str) -> Option<String> {
    with_feature_in(manifest, DEPENDENCIES, package, feature)
}

/// The one-line dependency on `package` under `section` with `feature` on.
pub(crate) fn with_feature_in(
    manifest: &str,
    section: &str,
    package: &str,
    feature: &str,
) -> Option<String> {
    let (start, end) = toml_section(manifest, section)?;
    let offset = start + dependency_line_start(&manifest[start..end], package)?;
    let line_end = manifest[offset..]
        .find('\n')
        .map_or(manifest.len(), |index| offset + index);
    let line = &manifest[offset..line_end];
    let dependency = without_toml_comment(line).trim_end();
    let edited = with_feature(dependency, &format!("\"{feature}\""))?;
    Some(format!(
        "{}{edited}{}{}",
        &manifest[..offset],
        &line[dependency.len()..],
        &manifest[line_end..]
    ))
}

/// A new app's workspace declares only `sofie` and `sofie-cli`, so a framework crate a
/// generator inherits joins `[workspace.dependencies]` from where they come: the workspace
/// manifest and its new text, or `None` when it declares the crate already.
pub(crate) fn workspace_declaration(
    directory: &Path,
    package: &str,
) -> Result<Option<(PathBuf, String)>, GenError> {
    const WORKSPACE_DEPENDENCIES: &str = "[workspace.dependencies]";
    let Some(manifest) = workspace_manifest(directory) else {
        return Ok(None);
    };
    let text = read(&manifest)?;
    let line = format!("{package} = {{ {} }}", framework_source(package));
    let Some((start, end)) = toml_section(&text, WORKSPACE_DEPENDENCIES) else {
        let table = format!("\n{WORKSPACE_DEPENDENCIES}\n{line}\n");
        return Ok(Some((manifest, insert_at(&text, None, &table))));
    };
    if dependency_line_start(&text[start..end], package).is_some() {
        return Ok(None);
    }
    let body = text[start..end].trim_end();
    let at = start + body.len();
    let inserted = if body.is_empty() {
        format!("{line}\n")
    } else {
        format!("\n{line}")
    };
    Ok(Some((
        manifest,
        format!("{}{inserted}{}", &text[..at], &text[at..]),
    )))
}

/// The nearest manifest at or above `directory` with a `[workspace]` table, as Cargo finds it.
fn workspace_manifest(directory: &Path) -> Option<PathBuf> {
    directory
        .ancestors()
        .map(|directory| directory.join("Cargo.toml"))
        .find(|manifest| {
            std::fs::read_to_string(manifest)
                .is_ok_and(|text| text.lines().any(|line| line.trim_end() == "[workspace]"))
        })
}

pub(crate) fn dependency_line_start(section: &str, package: &str) -> Option<usize> {
    let mut offset = 0;
    for line in section.split_inclusive('\n') {
        let named = line
            .strip_prefix(package)
            .is_some_and(|rest| rest.starts_with([' ', '=', '.']));
        if named {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

fn with_feature(line: &str, feature: &str) -> Option<String> {
    if let Some(package) = line.trim_end().strip_suffix(".workspace = true") {
        return Some(format!(
            "{package} = {{ workspace = true, features = [{feature}] }}"
        ));
    }
    match inline_features(line) {
        InlineFeatures::Array(array) => {
            let listed = &line[array.start + 1..array.end - 1];
            if listed.split(',').any(|listed| listed.trim() == feature) {
                return Some(line.to_owned());
            }
            let separator = if listed.trim().is_empty() { "" } else { ", " };
            let close = array.end - 1;
            Some(format!(
                "{}{separator}{feature}{}",
                line[..close].trim_end(),
                &line[close..]
            ))
        }
        InlineFeatures::Unreadable => None,
        InlineFeatures::Absent => {
            let close = line.rfind('}')?;
            Some(format!(
                "{}, features = [{feature}] {}",
                line[..close].trim_end(),
                &line[close..]
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::with_feature_on;

    #[test]
    fn turns_a_feature_on_in_any_form_of_the_sofie_dependency() {
        for (dependency, expected) in [
            (
                "sofie.workspace = true",
                "sofie = { workspace = true, features = [\"tasks\"] }",
            ),
            (
                "sofie = { workspace = true, features = [\"config\"] }",
                "sofie = { workspace = true, features = [\"config\", \"tasks\"] }",
            ),
            (
                "sofie = { workspace = true, features = [] }",
                "sofie = { workspace = true, features = [\"tasks\"] }",
            ),
            (
                "sofie = { path = \"../sofie\" }",
                "sofie = { path = \"../sofie\", features = [\"tasks\"] }",
            ),
            (
                "sofie = { workspace = true, features = [\"tasks\"] }",
                "sofie = { workspace = true, features = [\"tasks\"] }",
            ),
            (
                "sofie = { workspace = true, features=[\"config\"] }",
                "sofie = { workspace = true, features=[\"config\", \"tasks\"] }",
            ),
            (
                "sofie = { workspace = true, features = [\"tasks-extra\"] }",
                "sofie = { workspace = true, features = [\"tasks-extra\", \"tasks\"] }",
            ),
        ] {
            let manifest = format!(
                "[package]\nname = \"billing\"\n\n[dependencies]\nsofie-identity.workspace = true\n{dependency}\nserde.workspace = true\n"
            );
            assert_eq!(
                with_feature_on(&manifest, "sofie", "tasks").unwrap(),
                manifest.replace(dependency, expected),
                "{dependency}"
            );
        }
        assert_eq!(
            with_feature_on("[dependencies]\nserde.workspace = true\n", "sofie", "tasks"),
            None
        );
    }

    #[test]
    fn a_trailing_comment_stays_after_the_dependency_it_notes() {
        let commented = "[dependencies]\nsofie-identity.workspace = true # members sign in\r\n";
        let hashed = "[dependencies]\nsofie = { path = \"../so#fie\" } # \"permissions\"\n";

        assert_eq!(
            with_feature_on(commented, "sofie-identity", "permissions").unwrap(),
            "[dependencies]\nsofie-identity = { workspace = true, features = [\"permissions\"] } \
             # members sign in\r\n"
        );
        assert_eq!(
            with_feature_on(hashed, "sofie", "permissions").unwrap(),
            "[dependencies]\nsofie = { path = \"../so#fie\", features = [\"permissions\"] } \
             # \"permissions\"\n"
        );
    }
}
