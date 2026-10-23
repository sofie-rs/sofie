//! A file is never touched once written; new or changed facts get a new file, named by the moment.

mod facts;
mod link_facts;
mod requests;
mod route_facts;

use std::fmt::{self, Write as _};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use sofie_check::Screens;
use sofie_core::Timestamp;
use sofie_model::naming::relative_path;
use sofie_model::{App, Module};

use crate::gen_command::{GenError, io_error};
use crate::new_command::edits::with_feature_in;
use crate::new_command::{dependency_line_start, workspace_declaration};
use crate::rustfmt::format_rust;
use crate::sorted;
use crate::source_edits::{
    DEPENDENCIES, DEV_DEPENDENCIES, Journal, files_with_extension, path_dependency, read,
    toml_section, with_section_line,
};
use facts::{Fact, module_facts};

const DIRECTORY: [&str; 2] = ["tests", "scaffolded"];
const ROOT_FILE: &str = "main.rs";
/// Files written before the facts became the `_FACTS` constant list them on these lines.
const FACT_LINE: &str = "//! fact `";
const FACTS_CONSTANT: &str = "_FACTS";
const FILE_MOMENT: &str = "%Y%m%d%H%M%S";
const HEADER_MOMENT: &str = "%Y-%m-%d %H:%M:%S";

/// A macro, so that constant descriptions can `concat!` it.
macro_rules! test_places {
    () => {
        "Scaffolded tests live in `app/tests/scaffolded/`, one file per run; extend them there; \
         hand-written tests go in `app/tests/<topic>.rs`. Write each as \
         `#[test_app] async fn name(app: TestApp)`, as the scaffolded ones are; \
         `#[tokio::test]` only where `#[test_app]` cannot run it, with a line saying why."
    };
}
pub(crate) use test_places;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ScaffoldedTests {
    pub created: Vec<PathBuf>,
    pub changed: Vec<PathBuf>,
    pub superseded: Vec<String>,
    /// Gaps of the facts this run adds, or of subjects no test covers at all.
    pub uncovered: Vec<String>,
    /// Gaps of subjects earlier files already test, left as they were.
    pub older_uncovered: Vec<String>,
}

impl ScaffoldedTests {
    /// The older gaps counted on one line, as an agent asked for this run's tests reads them.
    pub(crate) fn summary(&self) -> String {
        let mut text = String::new();
        let _ = self.write(&mut text, false);
        text
    }

    fn write(&self, out: &mut impl fmt::Write, every_gap: bool) -> fmt::Result {
        for file in &self.created {
            writeln!(out, "created {}", file.display())?;
        }
        for file in &self.changed {
            writeln!(out, "changed {}", file.display())?;
        }
        for test in &self.superseded {
            writeln!(out, "superseded: {test}")?;
        }
        for reason in &self.uncovered {
            writeln!(out, "not covered: {reason}")?;
        }
        if every_gap {
            for reason in &self.older_uncovered {
                writeln!(out, "not covered: {reason}")?;
            }
        } else if !self.older_uncovered.is_empty() {
            let gaps = match self.older_uncovered.len() {
                1 => "1 older gap".to_owned(),
                count => format!("{count} older gaps"),
            };
            writeln!(
                out,
                "not covered: {gaps} this run leaves as they were; `scaffold_tests` with `all: true` lists them"
            )?;
        }
        if !self.created.is_empty() {
            writeln!(out, test_places!())?;
        }
        Ok(())
    }
}

impl fmt::Display for ScaffoldedTests {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write(formatter, true)
    }
}

struct Written {
    file: PathBuf,
    module: String,
    facts: Vec<(String, String)>,
}

pub fn scaffold_tests(root: &Path) -> Result<ScaffoldedTests, GenError> {
    scaffold_tests_at(root, Timestamp::now())
}

/// A dry run's moment is not the real run's, so its report names the moment's form instead.
pub(crate) fn without_moment(report: &str, now: Timestamp) -> String {
    let undated = report
        .replace(&now.strftime(FILE_MOMENT).to_string(), "<YYYYMMDDHHMMSS>")
        .replace(
            &now.strftime(HEADER_MOMENT).to_string(),
            "<YYYY-MM-DD HH:MM:SS>",
        );
    if undated == report {
        return undated;
    }
    format!(
        "{undated}\n<YYYYMMDDHHMMSS> is the moment the real run writes the file; it prints the name.\n"
    )
}

pub(crate) fn scaffold_tests_at(root: &Path, now: Timestamp) -> Result<ScaffoldedTests, GenError> {
    let app = sofie_model::load(root).map_err(GenError::Model)?;
    let directory = DIRECTORY
        .iter()
        .fold(app.app.path.clone(), |path, part| path.join(part));
    let written = read_written(&directory)?;
    let screens = Screens::read(&app.modules);
    let mut outcome = ScaffoldedTests::default();
    let mut journal = Journal::default();
    let scaffolded = sorted::modules(&app)
        .into_iter()
        .try_for_each(|module| {
            scaffold_module(
                &app,
                module,
                &screens,
                &directory,
                &written,
                now,
                &mut journal,
                &mut outcome,
            )
        })
        .and_then(|()| write_root(&directory, &mut journal, &mut outcome))
        .and_then(|()| add_test_dependencies(&app, &directory, &mut journal, &mut outcome));
    journal.finish(scaffolded)?;
    Ok(outcome)
}

const TESTING_PACKAGE: &str = "sofie-testing";
const IDENTITY_PACKAGE: &str = "sofie-identity";

/// What the scaffolded files name besides the app and its modules, which a new app lacks.
fn add_test_dependencies(
    app: &App,
    directory: &Path,
    journal: &mut Journal,
    outcome: &mut ScaffoldedTests,
) -> Result<(), GenError> {
    let mut sources = String::new();
    for file in files_with_extension(directory, "rs")? {
        sources.push_str(&fs::read_to_string(&file).map_err(|error| io_error(&file, error))?);
    }
    let manifest = app.app.path.join("Cargo.toml");
    let original = read(&manifest)?;
    let mut text = with_contracts(app, &sources, &original)?;
    if sources.contains("sofie_testing::") && !names_dependency(&text, TESTING_PACKAGE) {
        let line = format!("{TESTING_PACKAGE}.workspace = true\n");
        text = with_workspace_dev_dependency(app, &text, TESTING_PACKAGE, &line, journal, outcome)?;
    }
    let dev_names = |package| {
        toml_section(&text, DEV_DEPENDENCIES)
            .is_some_and(|(start, end)| dependency_line_start(&text[start..end], package).is_some())
    };
    if sources.contains("sofie_identity::testing::") && !dev_names(IDENTITY_PACKAGE) {
        let line =
            format!("{IDENTITY_PACKAGE} = {{ workspace = true, features = [\"testing\"] }}\n");
        text =
            with_workspace_dev_dependency(app, &text, IDENTITY_PACKAGE, &line, journal, outcome)?;
    }
    // `granted` writes roles and grants, which only sofie-identity's `permissions` has.
    if sources.contains(requests::GRANTED)
        && let Some(edited) =
            with_feature_in(&text, DEV_DEPENDENCIES, IDENTITY_PACKAGE, "permissions")
    {
        text = edited;
    }
    if text != original {
        journal.write(&manifest, &text)?;
        outcome.changed.push(manifest);
    }
    Ok(())
}

/// The contracts whose route builders the requests name.
fn with_contracts(app: &App, sources: &str, manifest: &str) -> Result<String, GenError> {
    let mut text = manifest.to_owned();
    for module in sorted::modules(app) {
        let Some(contract) = &module.contract else {
            continue;
        };
        if !sources.contains(&format!("{}::", contract.crate_name))
            || names_dependency(&text, &contract.package)
        {
            continue;
        }
        let path = relative_path(&app.app.path, &contract.path).ok_or_else(|| {
            GenError::Usage(format!(
                "{} cannot name {} by a relative path",
                app.app.path.display(),
                contract.path.display()
            ))
        })?;
        let line = path_dependency(&contract.package, &path);
        text = with_section_line(&text, DEV_DEPENDENCIES, &line);
    }
    Ok(text)
}

fn with_workspace_dev_dependency(
    app: &App,
    manifest: &str,
    package: &str,
    line: &str,
    journal: &mut Journal,
    outcome: &mut ScaffoldedTests,
) -> Result<String, GenError> {
    if let Some((workspace, declared)) = workspace_declaration(&app.app.path, package)? {
        journal.write(&workspace, declared)?;
        outcome.changed.push(workspace);
    }
    Ok(with_section_line(manifest, DEV_DEPENDENCIES, line))
}

fn names_dependency(manifest: &str, package: &str) -> bool {
    [DEPENDENCIES, DEV_DEPENDENCIES].iter().any(|header| {
        toml_section(manifest, header).is_some_and(|(start, end)| {
            dependency_line_start(&manifest[start..end], package).is_some()
        })
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "each is one run's state, passed down unchanged"
)]
fn scaffold_module(
    app: &App,
    module: &Module,
    screens: &Screens,
    directory: &Path,
    written: &[Written],
    now: Timestamp,
    journal: &mut Journal,
    outcome: &mut ScaffoldedTests,
) -> Result<(), GenError> {
    let name = &module.krate.crate_name;
    let found = module_facts(app, module, screens);
    let earlier: Vec<&Written> = written.iter().filter(|file| &file.module == name).collect();
    let covered = |text: &str| {
        earlier
            .iter()
            .any(|file| file.facts.iter().any(|(fact, _)| fact == text))
    };
    let new: Vec<&Fact> = found
        .facts
        .iter()
        .filter(|fact| !covered(&fact.text))
        .collect();
    for file in &earlier {
        for (fact, test) in &file.facts {
            if found.facts.iter().all(|current| &current.text != fact) {
                outcome
                    .superseded
                    .push(format!("{}: {test} ({fact})", file.file.display()));
            }
        }
    }
    for gap in &found.uncovered {
        let tested = |fact: &&Fact| fact.subject == gap.subject;
        let older = found.facts.iter().any(|fact| tested(&fact)) && !new.iter().any(tested);
        let reason = format!("{name}: {}", gap.reason);
        if older {
            outcome.older_uncovered.push(reason);
        } else {
            outcome.uncovered.push(reason);
        }
    }
    if new.is_empty() {
        return Ok(());
    }
    let file = directory.join(format!("{name}_{}.rs", now.strftime(FILE_MOMENT)));
    if file.exists() {
        return Err(GenError::Usage(format!(
            "{} exists; run again a second later",
            file.display()
        )));
    }
    let source = format_rust(&render(name, now, &new), &file).map_err(GenError::Rustfmt)?;
    journal.write(&file, &source)?;
    outcome.created.push(file);
    Ok(())
}

fn read_written(directory: &Path) -> Result<Vec<Written>, GenError> {
    let mut written = Vec::new();
    for file in files_with_extension(directory, "rs")? {
        let Some(stem) = file.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if stem == "main" {
            continue;
        }
        let Some((module, _)) = stem.rsplit_once('_') else {
            continue;
        };
        let text = fs::read_to_string(&file).map_err(|error| io_error(&file, error))?;
        let mut facts = constant_facts(&text);
        facts.extend(text.lines().filter_map(|line| {
            let (fact, named) = line.strip_prefix(FACT_LINE)?.rsplit_once("`: `")?;
            Some((fact.to_owned(), named.strip_suffix('`')?.to_owned()))
        }));
        written.push(Written {
            module: module.to_owned(),
            file,
            facts,
        });
    }
    Ok(written)
}

/// The facts are data, not comments, so a scaffold keeps the app within its comment ratio.
fn constant_facts(source: &str) -> Vec<(String, String)> {
    let Ok(file) = syn::parse_file(source) else {
        return Vec::new();
    };
    let list = file.items.iter().find_map(|item| match item {
        syn::Item::Const(constant) if constant.ident == FACTS_CONSTANT => Some(&*constant.expr),
        _ => None,
    });
    let Some(syn::Expr::Reference(reference)) = list else {
        return Vec::new();
    };
    let syn::Expr::Array(array) = &*reference.expr else {
        return Vec::new();
    };
    array
        .elems
        .iter()
        .filter_map(|pair| match pair {
            syn::Expr::Tuple(tuple) => match (tuple.elems.first(), tuple.elems.last()) {
                (Some(fact), Some(test)) => Some((text_of(fact)?, text_of(test)?)),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn text_of(expression: &syn::Expr) -> Option<String> {
    match expression {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(text),
            ..
        }) => Some(text.value()),
        _ => None,
    }
}

/// The root file is the app's once written, so a later run only appends the `mod` lines it lacks.
fn write_root(
    directory: &Path,
    journal: &mut Journal,
    outcome: &mut ScaffoldedTests,
) -> Result<(), GenError> {
    if !directory.is_dir() {
        return Ok(());
    }
    let modules: Vec<String> = files_with_extension(directory, "rs")?
        .iter()
        .filter_map(|file| file.file_stem()?.to_str().map(str::to_owned))
        .filter(|stem| stem != "main")
        .collect();
    let file = directory.join(ROOT_FILE);
    let existing = match fs::read_to_string(&file) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(io_error(&file, error)),
    };
    let (mut root, declared) = match &existing {
        Some(text) => (text.clone(), declared_modules(text, &file)?),
        None => (
            String::from(
                "//! @generated by `sofie scaffold tests`, which declares each file beside it; the files are yours.\n\n",
            ),
            Vec::new(),
        ),
    };
    let missing: Vec<&String> = modules
        .iter()
        .filter(|module| !declared.contains(module))
        .collect();
    if missing.is_empty() && existing.is_some() {
        return Ok(());
    }
    if !root.is_empty() && !root.ends_with('\n') {
        root.push('\n');
    }
    for module in missing {
        let _ = writeln!(root, "mod {module};");
    }
    journal.write(&file, &root)?;
    match existing {
        Some(_) => outcome.changed.push(file),
        None => outcome.created.push(file),
    }
    Ok(())
}

fn declared_modules(text: &str, file: &Path) -> Result<Vec<String>, GenError> {
    let parsed = syn::parse_file(text)
        .map_err(|error| GenError::App(format!("{}: {error}", file.display())))?;
    Ok(parsed
        .items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Mod(module) => Some(module.ident.to_string()),
            _ => None,
        })
        .collect())
}

fn render(module: &str, now: Timestamp, facts: &[&Fact]) -> String {
    let mut source = format!(
        "//! Scaffolded by `sofie scaffold tests` at {} UTC for module `{module}`; yours from now on. \
         A later run writes new or changed facts to a new file and names the tests of `{FACTS_CONSTANT}` it supersedes.\n\n\
         const {FACTS_CONSTANT}: &[(&str, &str)] = &[\n",
        now.strftime(HEADER_MOMENT)
    );
    let mut names: Vec<String> = Vec::new();
    for fact in facts {
        let mut name = fact.test.clone();
        let mut suffix = 2;
        while names.contains(&name) {
            name = format!("{}_{suffix}", fact.test);
            suffix += 1;
        }
        let _ = writeln!(source, "    ({:?}, {name:?}),", fact.text);
        names.push(name);
    }
    source.push_str("];\n");
    if facts.iter().any(|fact| fact.on_app) {
        source.push('\n');
        if facts
            .iter()
            .any(|fact| fact.body.contains(requests::SIGNED_UP_MEMBER))
        {
            source.push_str("use sofie_identity::testing::TestMember as _;\n");
        }
        if facts.iter().any(|fact| fact.body.contains(".await.id()")) {
            source.push_str("use sofie::db::Aggregate as _;\n");
        }
        source.push_str("use sofie_testing::{TestApp, test_app};\n");
    }
    for (fact, name) in facts.iter().zip(&names) {
        if fact.on_app {
            let _ = write!(
                source,
                "\n#[test_app]\nasync fn {name}(app: TestApp) {{\n{}\n}}\n",
                fact.body
            );
        } else {
            let _ = write!(source, "\n#[test]\nfn {name}() {{\n{}\n}}\n", fact.body);
        }
    }
    source
}

#[cfg(test)]
mod tests;
