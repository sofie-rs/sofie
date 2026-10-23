use std::fs;
use std::ops::RangeInclusive;
use std::path::Path;

use sofie_model::naming::{file_stem, screen_key};
use sofie_model::{App, Crate};

use super::ToolOutput;
use crate::check_command::check_app;
use crate::docs::{DOCS_HEADER, render_docs};
use crate::explain::{
    aggregate_section, explain as explain_name, module_section, read_model, screen,
};
use crate::gen_command::GenError;
use crate::routes_command::route_table;
use crate::sorted;
use crate::tasks_command::task_table;

pub(super) fn explain(root: &Path, name: &str) -> ToolOutput {
    with_model(root, |app| {
        let sections = explain_name(app, name);
        if sections.is_empty() {
            return ToolOutput::failed(format!(
                "nothing in the app is named `{name}`; the `modules`, `routes` and `screens` \
                 tools list what is"
            ));
        }
        ToolOutput::done(sections.join("\n\n"))
    })
}

pub(super) fn modules(root: &Path) -> ToolOutput {
    with_model(root, |app| {
        let sections: Vec<String> = std::iter::once(crate::explain::app_section(app))
            .chain(
                sorted::modules(app)
                    .into_iter()
                    .map(|module| module_section(app, module)),
            )
            .collect();
        ToolOutput::done(sections.join("\n\n"))
    })
}

pub(super) fn routes(root: &Path) -> ToolOutput {
    with_model(root, |app| match route_table(app) {
        Ok(table) => ToolOutput::done(table),
        Err(error) => ToolOutput::from_error(root, &GenError::Model(error)),
    })
}

pub(super) fn tasks(root: &Path) -> ToolOutput {
    with_model(root, |app| ToolOutput::done(task_table(app)))
}

pub(super) fn schema(root: &Path) -> ToolOutput {
    with_model(root, |app| {
        let sites = app.route_sites();
        let mut sections = Vec::new();
        for module in sorted::modules(app) {
            sections.extend(
                module
                    .code
                    .aggregates
                    .iter()
                    .map(|aggregate| aggregate_section(module, aggregate)),
            );
            sections.extend(
                module
                    .code
                    .read_models
                    .iter()
                    .map(|model| read_model(module, model, &sites)),
            );
            let mut migrations = vec![format!("migrations of {}", module.krate.package)];
            migrations.extend(
                module
                    .migrations
                    .iter()
                    .map(|migration| format!("  {}", migration.display())),
            );
            sections.push(migrations.join("\n"));
        }
        ToolOutput::done(or_none(&sections, "no modules"))
    })
}

pub(super) fn screens(root: &Path) -> ToolOutput {
    with_model(root, |app| {
        let sites = app.route_sites();
        let mut sections = Vec::new();
        for module in sorted::modules(app) {
            for area in &module.areas {
                sections.extend(area.screens.iter().filter_map(|file| {
                    let key = screen_key(&module.krate.crate_name, &area.area, &file_stem(file));
                    screen(module, &key, &sites)
                }));
            }
        }
        ToolOutput::done(or_none(&sections, "no screen templates"))
    })
}

pub(super) fn check(root: &Path) -> ToolOutput {
    match check_app(root) {
        Ok(findings) => {
            ToolOutput::judged(sofie_check::report(root, &findings), findings.is_empty())
        }
        Err(error) => ToolOutput::from_error(root, &error),
    }
}

pub(super) fn docs_page(root: &Path, page: &str) -> ToolOutput {
    with_model(root, |app| {
        let directory = app.root.join("docs").join("gen");
        let pages = render_docs(app);
        let wanted = directory.join(format!("{page}.md"));
        if let Some((_, text)) = pages.iter().find(|(path, _)| *path == wanted) {
            return ToolOutput::done(text.strip_prefix(DOCS_HEADER).unwrap_or(text).to_owned());
        }
        let names: Vec<String> = pages
            .iter()
            .filter_map(|(path, _)| path.strip_prefix(&directory).ok())
            .map(|path| path.with_extension("").display().to_string())
            .collect();
        ToolOutput::failed(format!(
            "no docs page `{page}`; the pages are {}",
            names.join(", ")
        ))
    })
}

pub(super) fn tests(root: &Path, name: Option<&str>) -> ToolOutput {
    with_model(root, |app| {
        let files = app_crates(app)
            .flat_map(|krate| krate.sources.iter().chain(&krate.tests_and_examples))
            .map(|source| source.file.as_path());
        if let Some(name) = name {
            return test_source(files, name);
        }
        let sections: Vec<String> = files.filter_map(test_section).collect();
        ToolOutput::done(or_none(&sections, "no tests"))
    })
}

/// Every test whose path is `name` or ends in `::name`, with the file and lines it spans.
fn test_source<'files>(files: impl Iterator<Item = &'files Path>, name: &str) -> ToolOutput {
    let mut sections = Vec::new();
    for file in files {
        let Ok(tests) = test_functions(file) else {
            continue;
        };
        let named = tests
            .iter()
            .filter(|test| test.path == name || test.path.ends_with(&format!("::{name}")));
        for test in named {
            match fs::read_to_string(file) {
                Ok(text) => sections.push(function_source(file, &text, &test.lines)),
                Err(error) => {
                    return ToolOutput::failed(format!(
                        "{} cannot be read: {error}",
                        file.display()
                    ));
                }
            }
        }
    }
    if sections.is_empty() {
        return ToolOutput::failed(format!(
            "no test is named `{name}`; `tests` without `name` lists them"
        ));
    }
    ToolOutput::done(sections.join("\n\n"))
}

fn function_source(file: &Path, text: &str, lines: &RangeInclusive<usize>) -> String {
    let source: Vec<&str> = text
        .lines()
        .skip(lines.start() - 1)
        .take(lines.end() + 1 - lines.start())
        .collect();
    format!(
        "{}:{}-{}\n{}",
        file.display(),
        lines.start(),
        lines.end(),
        source.join("\n")
    )
}

pub(super) fn app_crates(app: &App) -> impl Iterator<Item = &Crate> {
    let modules = sorted::modules(app).into_iter();
    std::iter::once(&app.app)
        .chain(modules.flat_map(|module| std::iter::once(&module.krate).chain(&module.contract)))
}

/// `None` for a file without tests.
fn test_section(file: &Path) -> Option<String> {
    let mut lines = vec![file.display().to_string()];
    match test_functions(file) {
        Ok(tests) => lines.extend(
            tests
                .iter()
                .map(|test| format!("  #[{}] {}", test.attribute, test.path)),
        ),
        Err(error) => lines.push(format!("  cannot be read: {error}")),
    }
    (lines.len() > 1).then(|| lines.join("\n"))
}

pub(super) struct TestFunction {
    attribute: String,
    /// Within the file, as `tests::adds`.
    pub(super) path: String,
    /// One-based, from its first attribute through its closing brace.
    lines: RangeInclusive<usize>,
}

pub(super) fn test_functions(file: &Path) -> Result<Vec<TestFunction>, String> {
    let text = fs::read_to_string(file).map_err(|error| error.to_string())?;
    let parsed = syn::parse_file(&text).map_err(|error| error.to_string())?;
    let mut tests = Vec::new();
    collect_tests(&parsed.items, "", &mut tests);
    Ok(tests)
}

fn collect_tests(items: &[syn::Item], prefix: &str, tests: &mut Vec<TestFunction>) {
    for item in items {
        match item {
            syn::Item::Fn(function) => {
                tests.extend(
                    test_attribute(&function.attrs).map(|attribute| TestFunction {
                        attribute,
                        path: format!("{prefix}{}", function.sig.ident),
                        lines: function_lines(function),
                    }),
                );
            }
            syn::Item::Mod(syn::ItemMod {
                ident,
                content: Some((_, items)),
                ..
            }) => collect_tests(items, &format!("{prefix}{ident}::"), tests),
            _ => {}
        }
    }
}

fn function_lines(function: &syn::ItemFn) -> RangeInclusive<usize> {
    let first = function.attrs.first().map_or_else(
        || function.sig.fn_token.span.start().line,
        |attribute| attribute.pound_token.span.start().line,
    );
    first..=function.block.brace_token.span.close().end().line
}

/// `#[test]`, `#[tokio::test]` and the like, or a `#[test_…]` macro of a testing crate.
fn test_attribute(attributes: &[syn::Attribute]) -> Option<String> {
    let path = attributes.iter().map(syn::Attribute::path).find(|path| {
        path.segments.last().is_some_and(|last| {
            let name = last.ident.to_string();
            name == "test" || name.starts_with("test_")
        })
    })?;
    let segments: Vec<String> = path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect();
    Some(segments.join("::"))
}

fn with_model(root: &Path, answer: impl FnOnce(&App) -> ToolOutput) -> ToolOutput {
    match sofie_model::load(root) {
        Ok(app) => answer(&app),
        Err(error) => ToolOutput::from_error(root, &GenError::Model(error)),
    }
}

fn or_none(sections: &[String], none: &str) -> String {
    if sections.is_empty() {
        return none.to_owned();
    }
    sections.join("\n\n")
}
