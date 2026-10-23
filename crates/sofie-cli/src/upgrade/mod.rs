//! Codemods for the framework's breaking changes. Each rewrites only the
//! old form of an API, so running it on code already upgraded changes nothing
//! and an app needs no record of which ones ran.

mod content_security;
mod features;
mod password;

use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::LineColumn;
use sofie_check::Finding;
use sofie_model::Crate;

use crate::check_command::check_app;
use crate::gen_command::{GenError, io_error, regenerate};
use crate::new_command::{dependency_line_start, workspace_declaration};
use crate::source_edits::{DEPENDENCIES, Journal, read, toml_section, with_dependency_line};

struct Codemod {
    name: &'static str,
    /// The commit whose breaking change it carries apps across.
    change: &'static str,
    edits: fn(&str, &syn::File) -> Vec<Edit>,
    /// The framework package the rewritten code names, which the crate then depends on.
    needs: Option<&'static str>,
}

const CODEMODS: &[Codemod] = &[
    Codemod {
        name: "content-security-on-serving",
        change: "e586a89: content security sources are set on `Serving`, not `App`",
        edits: content_security::edits,
        needs: None,
    },
    Codemod {
        name: "password-from-identity",
        change: "1eb6a1d: `Password` and `PasswordHash` come from sofie_identity",
        edits: password::edits,
        needs: Some("sofie-identity"),
    },
];

/// Replaces `start..end` of a file's text.
pub(crate) struct Edit {
    start: LineColumn,
    end: LineColumn,
    text: String,
}

impl Edit {
    fn spanning(spanned: &impl syn::spanned::Spanned, text: String) -> Self {
        let span = spanned.span();
        Self {
            start: span.start(),
            end: span.end(),
            text,
        }
    }

    fn insert(at: LineColumn, text: String) -> Self {
        Self {
            start: at,
            end: at,
            text,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    pub file: PathBuf,
    pub codemod: &'static str,
    pub change: &'static str,
    pub line: usize,
    pub before: String,
    pub after: String,
}

pub struct UpgradeReport {
    pub changes: Vec<Change>,
    /// After writing: what `sofie check` still finds once `sofie gen` ran.
    pub remaining: Option<Vec<Finding>>,
    /// A dry run that would upgrade manifests plans no source change: the model is read once
    /// they are written.
    pub sources_pending: bool,
}

/// Without `write`, shows what each codemod would change and touches nothing.
pub fn upgrade(root: &Path, write: bool) -> Result<UpgradeReport, GenError> {
    // `cargo metadata`, and so the model, refuses the app until its manifests are upgraded.
    let manifests = features::planned(root)?;
    if !write && !manifests.changes.is_empty() {
        return Ok(UpgradeReport {
            changes: manifests.changes,
            remaining: None,
            sources_pending: true,
        });
    }
    let mut journal = Journal::default();
    let written = write_upgrade(&mut journal, root, manifests, write);
    let changes = journal.finish(written)?;
    let remaining = if write && !changes.is_empty() {
        Some(check_app(root)?)
    } else {
        None
    };
    Ok(UpgradeReport {
        changes,
        remaining,
        sources_pending: false,
    })
}

/// Every source file is parsed and upgraded before the first is written; the caller's
/// journal undoes them all when anything after fails, gen included.
fn write_upgrade(
    journal: &mut Journal,
    root: &Path,
    manifests: Planned,
    write: bool,
) -> Result<Vec<Change>, GenError> {
    for (file, upgraded) in &manifests.files {
        journal.write(file, upgraded)?;
    }
    let mut changes = manifests.changes;
    let planned = plan(&sofie_model::load(root).map_err(GenError::Model)?)?;
    changes.extend(planned.changes);
    if !write || changes.is_empty() {
        return Ok(changes);
    }
    for (file, upgraded) in &planned.files {
        journal.write(file, upgraded)?;
    }
    regenerate(root)?;
    Ok(changes)
}

fn plan(app: &sofie_model::App) -> Result<Planned, GenError> {
    let mut planned = Planned::default();
    for krate in crates(app) {
        for (package, codemod) in planned.upgrade_files(krate)? {
            planned.depend(krate, package, codemod)?;
            // `sofie gen` wires a framework crate a module uses only through the app.
            planned.depend(&app.app, package, codemod)?;
        }
    }
    Ok(planned)
}

fn codemod(name: &str) -> &'static Codemod {
    CODEMODS
        .iter()
        .find(|codemod| codemod.name == name)
        .expect("a change names the codemod making it")
}

/// What the upgrade would write, each file once with all its changes.
#[derive(Default)]
struct Planned {
    changes: Vec<Change>,
    files: Vec<(PathBuf, String)>,
}

impl Planned {
    /// The packages the rewritten code of `krate` names, with a codemod naming each.
    fn upgrade_files(
        &mut self,
        krate: &Crate,
    ) -> Result<Vec<(&'static str, &'static Codemod)>, GenError> {
        let mut needed = Vec::new();
        for file in rust_files(krate) {
            if self.files.iter().any(|(planned, _)| *planned == file) {
                continue;
            }
            let text = fs::read_to_string(&file).map_err(|source| io_error(&file, source))?;
            let (upgraded, found) = upgrade_text(&text)
                .map_err(|error| GenError::App(format!("{}: {error}", file.display())))?;
            if found.is_empty() {
                continue;
            }
            for change in found {
                let codemod = codemod(change.codemod);
                if let Some(package) = codemod.needs
                    && needed.iter().all(|(needed, _)| *needed != package)
                {
                    needed.push((package, codemod));
                }
                self.changes.push(Change {
                    file: file.clone(),
                    ..change
                });
            }
            self.files.push((file, upgraded));
        }
        Ok(needed)
    }

    fn depend(&mut self, krate: &Crate, package: &str, codemod: &Codemod) -> Result<(), GenError> {
        if krate.depends_on(package) {
            return Ok(());
        }
        let manifest = krate.path.join("Cargo.toml");
        let text = self.text(&manifest)?;
        let declared = toml_section(&text, DEPENDENCIES).is_some_and(|(start, end)| {
            dependency_line_start(&text[start..end], package).is_some()
        });
        if declared {
            return Ok(());
        }
        let line = format!("{package}.workspace = true\n");
        self.plan(manifest, with_dependency_line(&text, &line), &line, codemod);
        let Some((workspace, text)) = workspace_declaration(&krate.path, package)? else {
            return Ok(());
        };
        if self.files.iter().all(|(planned, _)| *planned != workspace) {
            let line = text
                .lines()
                .find(|line| dependency_line_start(line, package).is_some())
                .unwrap_or_default()
                .to_owned();
            self.plan(workspace, text, &line, codemod);
        }
        Ok(())
    }

    fn text(&self, file: &Path) -> Result<String, GenError> {
        match self.files.iter().find(|(planned, _)| planned == file) {
            Some((_, text)) => Ok(text.clone()),
            None => read(file),
        }
    }

    fn plan(&mut self, file: PathBuf, text: String, added: &str, codemod: &Codemod) {
        let added = added.trim_end();
        let line = text
            .lines()
            .position(|line| line == added)
            .map_or(0, |index| index + 1);
        self.changes.push(Change {
            file: file.clone(),
            codemod: codemod.name,
            change: codemod.change,
            line,
            before: String::new(),
            after: added.to_owned(),
        });
        match self.files.iter_mut().find(|(planned, _)| *planned == file) {
            Some((_, planned)) => *planned = text,
            None => self.files.push((file, text)),
        }
    }
}

fn crates(app: &sofie_model::App) -> impl Iterator<Item = &Crate> {
    std::iter::once(&app.app).chain(
        app.modules
            .iter()
            .flat_map(|module| std::iter::once(&module.krate).chain(&module.contract)),
    )
}

fn rust_files(krate: &Crate) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = krate
        .sources
        .iter()
        .chain(&krate.tests_and_examples)
        .map(|source| source.file.clone())
        .collect();
    files.sort();
    files.dedup();
    files
}

/// Codemods run one after another, each on the text the one before left.
fn upgrade_text(text: &str) -> syn::Result<(String, Vec<Change>)> {
    let mut text = text.to_owned();
    let mut file = syn::parse_file(&text)?;
    let mut changes = Vec::new();
    for codemod in CODEMODS {
        let mut edits = (codemod.edits)(&text, &file);
        if edits.is_empty() {
            continue;
        }
        edits.sort_by_key(|edit| (edit.start.line, edit.start.column));
        let upgraded = apply(&text, &edits);
        // Removed lines all lie before the edit, so the subtraction stays positive.
        let (mut added_before, mut removed_before) = (0, 0);
        for edit in &edits {
            let removed = edit.end.line - edit.start.line;
            let added = edit.text.matches('\n').count();
            let line = edit.start.line + added_before - removed_before;
            changes.push(Change {
                file: PathBuf::new(),
                codemod: codemod.name,
                change: codemod.change,
                line,
                before: lines(&text, edit.start.line, removed),
                after: lines(&upgraded, line, added),
            });
            added_before += added;
            removed_before += removed;
        }
        text = upgraded;
        file = syn::parse_file(&text)?;
    }
    Ok((text, changes))
}

/// Line `first` (from 1) and the `more` after it, each trimmed.
fn lines(text: &str, first: usize, more: usize) -> String {
    text.lines()
        .skip(first - 1)
        .take(more + 1)
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Edits never overlap; applied from the last, so earlier positions stay valid.
fn apply(text: &str, edits: &[Edit]) -> String {
    let mut text = text.to_owned();
    for edit in edits.iter().rev() {
        let start = offset(&text, edit.start);
        let end = offset(&text, edit.end);
        text.replace_range(start..end, &edit.text);
    }
    text
}

/// `column` counts characters, as proc-macro2 does.
fn offset(text: &str, at: LineColumn) -> usize {
    let line_start: usize = text
        .split_inclusive('\n')
        .take(at.line - 1)
        .map(str::len)
        .sum();
    line_start
        + text[line_start..]
            .chars()
            .take(at.column)
            .map(char::len_utf8)
            .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::upgrade_text;

    #[test]
    fn an_upgraded_text_is_left_as_it_is() {
        let text = "fn app(app: App) -> App {\n    app.serving(|serving| serving.content_security::<Web>(sources()))\n}\n";

        let (upgraded, changes) = upgrade_text(text).unwrap();

        assert_eq!(upgraded, text);
        assert_eq!(changes, []);
    }

    #[test]
    fn reports_each_change_with_its_line_before_and_after() {
        let text = "fn app(app: App) -> App {\n    app.content_security_all(sources())\n}\n";

        let (_, changes) = upgrade_text(text).unwrap();

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].line, 2);
        assert_eq!(changes[0].before, "app.content_security_all(sources())");
        assert_eq!(
            changes[0].after,
            "app.serving(|serving| serving.content_security_all(sources()))"
        );
    }
}
