use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sofie_check::{Finding, Screen, Screens};
use sofie_model::{App, ModelError};

use crate::agents_md::render_agents_md;
use crate::api::typescript::TYPESCRIPT_HEADER;
use crate::contract_gen::render_contract_gens;
use crate::env_example::{ENV_HEADER, render_env_examples};
use crate::fix_command::wire_generated_code;
use crate::forms::described_forms;
use crate::module_roles::{missing_role_migrations, with_role_migrations};
use crate::rustfmt::format_rust;
use crate::source_edits::write;
use crate::translations::AppLanguages;
use crate::wiring::{HEADER, render_module_wiring, render_wiring};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenMode {
    Write,
    Check,
}

#[derive(Debug, PartialEq, Eq)]
pub enum GenOutcome {
    Written(PathBuf),
    Removed(PathBuf),
    UpToDate(PathBuf),
    Stale(PathBuf),
}

#[derive(Debug)]
pub enum GenError {
    Model(ModelError),
    Check(Vec<Finding>),
    /// rustfmt is missing or refused the output: a broken toolchain, not a broken app.
    Rustfmt(String),
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Screen {
        path: PathBuf,
        message: String,
    },
    App(String),
    /// The app's own `cargo build` failed; the text holds its diagnostics.
    Build(String),
    Usage(String),
}

impl fmt::Display for GenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model(error) => write!(formatter, "{error}"),
            Self::Check(findings) => {
                let rendered: Vec<String> = findings.iter().map(ToString::to_string).collect();
                write!(formatter, "{}", rendered.join("\n"))
            }
            Self::Rustfmt(message)
            | Self::App(message)
            | Self::Build(message)
            | Self::Usage(message) => {
                write!(formatter, "{message}")
            }
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Screen { path, message } => write!(formatter, "{}: {message}", path.display()),
        }
    }
}

impl std::error::Error for GenError {}

impl GenOutcome {
    pub(crate) fn change(&self) -> Option<String> {
        match self {
            Self::Written(path) => Some(format!("wrote {}", path.display())),
            Self::Removed(path) => Some(format!("removed {}", path.display())),
            Self::UpToDate(_) | Self::Stale(_) => None,
        }
    }
}

/// Writing, it first makes the generated code reachable, as `sofie fix` would.
pub fn gen_app(root: &Path, mode: GenMode) -> Result<Vec<GenOutcome>, GenError> {
    generated_app(root, mode).map(|(_, outcomes)| outcomes)
}

/// The checked model the generated code was rendered from, so docs need not load it again.
pub(crate) fn generated_app(
    root: &Path,
    mode: GenMode,
) -> Result<(App, Vec<GenOutcome>), GenError> {
    let mut app = sofie_model::load(root).map_err(GenError::Model)?;
    let mut outcomes = Vec::new();
    if mode == GenMode::Write {
        let wired = wire_generated_code(&app)?;
        if !wired.is_empty() {
            app = sofie_model::load(root).map_err(GenError::Model)?;
            outcomes.extend(wired.into_iter().map(|(file, _)| GenOutcome::Written(file)));
        }
    }
    let (screens, languages) = checked_parts(&app)?;
    outcomes.extend(sync_generated(&app, &screens, languages.as_ref(), mode)?);
    Ok((app, outcomes))
}

pub fn regenerate(root: &Path) -> Result<Vec<String>, GenError> {
    let outcomes = regenerated(root, GenMode::Write)?;
    Ok(outcomes.iter().filter_map(GenOutcome::change).collect())
}

/// `sofie gen`: the generated code, then `docs/gen/` for an app keeping its docs, read from
/// the model the new code gives.
pub fn regenerated(root: &Path, mode: GenMode) -> Result<Vec<GenOutcome>, GenError> {
    let (app, mut outcomes) = generated_app(root, mode)?;
    let docs = regenerate_docs(&app, &outcomes, mode)?;
    outcomes.extend(docs);
    Ok(outcomes)
}

/// The model is loaded again only when the code just written can change it; that code was
/// rendered from a checked model, so it is not checked again.
#[cfg(feature = "docs")]
fn regenerate_docs(
    app: &App,
    outcomes: &[GenOutcome],
    mode: GenMode,
) -> Result<Vec<GenOutcome>, GenError> {
    if !crate::docs::keeps_docs(app) {
        return Ok(Vec::new());
    }
    if !outcomes.iter().any(|outcome| outcome.change().is_some()) {
        return crate::docs::sync_docs(app, mode);
    }
    let reloaded = sofie_model::load(&app.root).map_err(GenError::Model)?;
    crate::docs::sync_docs(&reloaded, mode)
}

#[cfg(not(feature = "docs"))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as with `docs`"
)]
fn regenerate_docs(
    _app: &App,
    _outcomes: &[GenOutcome],
    _mode: GenMode,
) -> Result<Vec<GenOutcome>, GenError> {
    Ok(Vec::new())
}

#[cfg(feature = "docs")]
pub(crate) fn checked_model(root: &Path) -> Result<App, GenError> {
    checked(root).map(|(app, _, _)| app)
}

#[cfg(feature = "docs")]
fn checked(root: &Path) -> Result<(App, Screens, Option<AppLanguages>), GenError> {
    let app = sofie_model::load(root).map_err(GenError::Model)?;
    let (screens, languages) = checked_parts(&app)?;
    Ok((app, screens, languages))
}

fn checked_parts(app: &App) -> Result<(Screens, Option<AppLanguages>), GenError> {
    let screens = Screens::read(&app.modules);
    let languages = AppLanguages::read(app)?;
    let mut findings = sofie_check::check(app, &screens);
    if let Some(languages) = &languages {
        findings.extend(languages.findings(app, &screens)?);
    }
    if !findings.is_empty() {
        return Err(GenError::Check(findings));
    }
    Ok((screens, languages))
}

pub(crate) fn sync_generated(
    app: &App,
    screens: &Screens,
    languages: Option<&AppLanguages>,
    mode: GenMode,
) -> Result<Vec<GenOutcome>, GenError> {
    let role_migrations = missing_role_migrations(app);
    let app = &with_role_migrations(app, &role_migrations);
    let mut files: Vec<(PathBuf, Option<String>)> = role_migrations
        .into_iter()
        .map(|(path, text)| (path, Some(text)))
        .collect();
    let wiring = render_wiring(app, languages, &described_forms(app, screens)?);
    let wiring_file = app.wiring_file();
    files.push((wiring_file.clone(), Some(formatted(&wiring, &wiring_file)?)));
    for module in &app.modules {
        let file = module.krate.wiring_file();
        let text = formatted(&render_module_wiring(app, module), &file)?;
        files.push((file, Some(text)));
    }
    let contract_gens = render_contract_gens(app, languages)
        .map_err(|error| GenError::App(format!("reading the translation keys: {error}")))?;
    for (path, text) in contract_gens {
        let text = text
            .as_deref()
            .map(|text| formatted(text, &path))
            .transpose()?;
        files.push((path, text));
    }
    files.extend(render_env_examples(app)?);
    #[cfg(feature = "typescript")]
    files.push(crate::islands::types_file(app));
    files.push(render_agents_md(app)?);
    let mut outcomes = Vec::new();
    for (path, text) in files {
        outcomes.extend(sync_file(&path, text, mode)?);
    }
    Ok(outcomes)
}

fn formatted(rust: &str, file: &Path) -> Result<String, GenError> {
    format_rust(rust, file).map_err(GenError::Rustfmt)
}

pub(crate) fn sync_file(
    path: &Path,
    text: Option<String>,
    mode: GenMode,
) -> Result<Option<GenOutcome>, GenError> {
    let existing = read_existing(path)?;
    let Some(text) = text else {
        return remove_generated(path, existing.as_deref(), mode);
    };
    if existing.as_deref() == Some(text.as_str()) {
        return Ok(Some(GenOutcome::UpToDate(path.to_path_buf())));
    }
    if mode == GenMode::Check {
        return Ok(Some(GenOutcome::Stale(path.to_path_buf())));
    }
    write(path, &text)?;
    Ok(Some(GenOutcome::Written(path.to_path_buf())))
}

/// Only a file `sofie gen` wrote is removed; anything else there is the developer's.
fn remove_generated(
    path: &Path,
    existing: Option<&str>,
    mode: GenMode,
) -> Result<Option<GenOutcome>, GenError> {
    if !existing.is_some_and(is_generated) {
        return Ok(None);
    }
    if mode == GenMode::Check {
        return Ok(Some(GenOutcome::Stale(path.to_path_buf())));
    }
    fs::remove_file(path).map_err(|source| io_error(path, source))?;
    Ok(Some(GenOutcome::Removed(path.to_path_buf())))
}

fn is_generated(text: &str) -> bool {
    #[cfg(feature = "docs")]
    if text.starts_with(crate::docs::DOCS_HEADER) {
        return true;
    }
    [HEADER, ENV_HEADER, TYPESCRIPT_HEADER]
        .iter()
        .any(|header| text.starts_with(header))
}

pub(crate) fn read_existing(path: &Path) -> Result<Option<String>, GenError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(io_error(path, source)),
    }
}

/// Cached, so a read error is copied out for the caller to own.
pub(crate) fn readable_screen<'s>(
    screens: &'s Screens,
    file: &Path,
) -> Result<&'s Screen, GenError> {
    screens
        .screen(file)
        .as_ref()
        .map_err(|error| io_error(file, io::Error::new(error.kind(), error.to_string())))
}

pub(crate) fn io_error(path: &Path, source: io::Error) -> GenError {
    GenError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::is_generated;
    use crate::api::typescript::TYPESCRIPT_HEADER;

    #[test]
    fn the_island_types_file_counts_as_generated() {
        assert!(is_generated(&format!("{TYPESCRIPT_HEADER}export {{}};\n")));
        assert!(!is_generated("// Written by hand.\nexport {};\n"));
    }
}
