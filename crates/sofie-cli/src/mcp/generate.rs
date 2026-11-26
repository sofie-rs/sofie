use std::fmt::Write as _;
use std::path::Path;

use sofie_core::Timestamp;

use super::ToolOutput;
use super::dry_run::{dry_report, dry_run};
use super::edit::{ScreenEdit, replace_once};
use crate::cargo::Diagnostics;
use crate::check_command::check_app;
use crate::feature_command::{FeatureChange, change_feature};
use crate::fix_command::fix_app;
use crate::gen_command::{GenError, GenOutcome, regenerate};
use crate::new_command::{NewCode, new_code, new_module};
use crate::scaffold_command::scaffold_screen_built_by;
use crate::scaffold_tests::{scaffold_module_tests_at, without_moment};

pub(super) enum Generator {
    NewModule(String),
    New(NewCode),
    ScaffoldScreen(String),
    EditScreen(ScreenEdit),
    /// The moment names the file it writes; `all` lists older gaps too; `module` limits the run.
    ScaffoldTests {
        now: Timestamp,
        all: bool,
        module: Option<String>,
    },
    Gen,
    Fix,
    Feature(String, FeatureChange),
}

pub(super) fn run(root: &Path, generator: &Generator, dry: bool) -> ToolOutput {
    if !dry {
        return generate(root, root, generator);
    }
    let ran = dry_run(root, |copy| {
        let mut output = generate(copy, root, generator);
        output.text = output
            .text
            .replace(&copy.display().to_string(), &root.display().to_string());
        output
    });
    match ran {
        Ok((mut output, changes)) => {
            output.text = dry_report(&output.text, &changes);
            if let Generator::ScaffoldTests { now, .. } = generator {
                output.text = without_moment(&output.text, *now);
            }
            output
        }
        Err(error) => ToolOutput::from_error(root, &error),
    }
}

/// `original` is the app itself, also when `root` is a dry run's copy of it.
fn generate(root: &Path, original: &Path, generator: &Generator) -> ToolOutput {
    let mut text = match step(root, original, generator) {
        Ok(report) => report,
        Err(error) => return ToolOutput::from_error(root, &error),
    };
    // `new_module` ends in its own `sofie gen`, already in its report, and `fix_app` runs
    // one whenever its checks find only stale generated code.
    if !matches!(generator, Generator::NewModule(_) | Generator::Fix) {
        match regenerate(root) {
            Ok(written) if written.is_empty() => {
                let _ = writeln!(text, "sofie gen: {NOTHING_TO_WRITE}");
            }
            Ok(written) => {
                let _ = writeln!(text, "sofie gen:\n  {}", written.join("\n  "));
            }
            Err(error) => {
                let failed = ToolOutput::from_error(root, &error);
                return ToolOutput::failed(format!(
                    "{text}sofie gen failed, and the changes above are kept; fix what it reports \
                     and run `gen`:\n{}",
                    failed.text
                ));
            }
        }
    }
    match check_app(root) {
        Ok(findings) => {
            let _ = write!(
                text,
                "sofie check:\n{}",
                sofie_check::report(root, &findings)
            );
            ToolOutput::judged(text, findings.is_empty())
        }
        Err(error) => {
            let failed = ToolOutput::from_error(root, &error);
            ToolOutput::failed(format!("{text}sofie check failed:\n{}", failed.text))
        }
    }
}

fn step(root: &Path, original: &Path, generator: &Generator) -> Result<String, GenError> {
    let lines = match generator {
        Generator::NewModule(name) => {
            let outcomes = new_module(root, name)?;
            vec![format!(
                "created module `{name}`; its first sofie gen:\n{}",
                outcome_lines(&outcomes)
            )]
        }
        Generator::New(code) => new_code(root, code)?
            .iter()
            .map(ToString::to_string)
            .collect(),
        Generator::ScaffoldScreen(key) => {
            let file = scaffold_screen_built_by(root, original, key, Diagnostics::Captured)?;
            vec![format!("wrote {}", file.display())]
        }
        Generator::EditScreen(edit) => {
            vec![format!("edited {}", replace_once(root, edit)?.display())]
        }
        Generator::ScaffoldTests { now, all, module } => {
            let scaffolded = scaffold_module_tests_at(root, *now, module.as_deref())?;
            return Ok(if *all {
                scaffolded.to_string()
            } else {
                scaffolded.summary()
            });
        }
        Generator::Gen => Vec::new(),
        Generator::Fix => fix_app(root)?.applied,
        Generator::Feature(name, change) => vec![change_feature(root, name, *change)?.to_string()],
    };
    let mut text = lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    Ok(text)
}

/// A route edit can leave nothing to write: a running `dev` or a `request` already
/// regenerated, or the edit touched only what the wiring does not carry.
const NOTHING_TO_WRITE: &str = "nothing to write; the generated files already match the code \
     (a running `dev` or a `request` regenerates them too)";

fn outcome_lines(outcomes: &[GenOutcome]) -> String {
    let lines: Vec<String> = outcomes
        .iter()
        .filter_map(GenOutcome::change)
        .map(|change| format!("  {change}"))
        .collect();
    if lines.is_empty() {
        return format!("  {NOTHING_TO_WRITE}");
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sofie_core::Timestamp;
    use sofie_model::testing::{TemporaryDirectory, manifest};

    use super::{Generator, step};

    fn module_with_an_aggregate(directory: &TemporaryDirectory, name: &str, aggregate: &str) {
        directory.write(&format!("modules/{name}/Cargo.toml"), &manifest(name, ""));
        directory.write(
            &format!("modules/{name}/src/lib.rs"),
            &format!(
                "use sofie::prelude::*;\n\n#[derive(Aggregate)]\npub struct {aggregate} {{\n    \
                 meta: Meta,\n    #[max(40)]\n    name: String,\n}}\n"
            ),
        );
    }

    fn at() -> Timestamp {
        Timestamp::from_second(1_791_720_000).unwrap()
    }

    /// A flagged route's test calls `app.flags()`, which plain sofie-testing lacks.
    #[test]
    fn a_flagged_route_s_test_turns_on_sofie_testing_s_flags() {
        let directory = TemporaryDirectory::new("sofie-mcp-scaffold-flags");
        directory.write(
            "Cargo.toml",
            "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"modules/*\", \"modules/*/contract\"]\n",
        );
        directory.write(
            "app/Cargo.toml",
            &manifest("shop", "catalog = { path = \"../modules/catalog\" }\n"),
        );
        directory.write("app/src/main.rs", "fn main() {}\n");
        directory.write("modules/catalog/Cargo.toml", &manifest("catalog", ""));
        directory.write(
            "modules/catalog/contract/Cargo.toml",
            &manifest("catalog-contract", ""),
        );
        directory.write(
            "modules/catalog/contract/src/lib.rs",
            "mod r#gen;\n\npub use r#gen::routes;\n",
        );
        directory.write(
            "modules/catalog/src/lib.rs",
            "use sofie::prelude::*;\n\nmod queries;\n\n\
             pub const PAGES: Feature = Feature::new(\"catalog.pages\");\n\n\
             pub mod areas {\n    pub mod web {\n        pub mod routes;\n    }\n}\n",
        );
        directory.write(
            "modules/catalog/src/queries.rs",
            "use sofie::prelude::*;\n\npub async fn list_pages() -> Result<Vec<String>> {\n    \
             Ok(Vec::new())\n}\n",
        );
        directory.write(
            "modules/catalog/src/areas/web/routes.rs",
            "use sofie::prelude::*;\n\nuse crate::queries;\n\npub fn routes(routes: &mut Routes<Web>) {\n    \
             routes.get(\"/pages\", queries::list_pages).feature(crate::PAGES);\n}\n",
        );
        let generator = Generator::ScaffoldTests {
            now: at(),
            all: true,
            module: None,
        };

        let report = step(directory.root(), directory.root(), &generator).unwrap();

        let written = fs::read_dir(directory.path("app/tests/scaffolded"))
            .unwrap()
            .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
            .collect::<String>();
        assert!(written.contains("app.flags()"), "{report}\n{written}");
        let app_manifest = fs::read_to_string(directory.path("app/Cargo.toml")).unwrap();
        assert!(
            app_manifest.contains("sofie-testing = { workspace = true, features = [\"flags\"] }"),
            "{app_manifest}"
        );
    }

    #[test]
    fn scaffolding_one_module_writes_only_its_tests() {
        let directory = TemporaryDirectory::new("sofie-mcp-scaffold-module");
        directory.write(
            "Cargo.toml",
            "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"modules/*\"]\n",
        );
        directory.write(
            "app/Cargo.toml",
            &manifest(
                "shop",
                "catalog = { path = \"../modules/catalog\" }\nstock = { path = \"../modules/stock\" }\n",
            ),
        );
        directory.write("app/src/main.rs", "fn main() {}\n");
        module_with_an_aggregate(&directory, "catalog", "Product");
        module_with_an_aggregate(&directory, "stock", "Shelf");
        let generator = Generator::ScaffoldTests {
            now: at(),
            all: false,
            module: Some("stock".to_owned()),
        };

        let report = step(directory.root(), directory.root(), &generator).unwrap();

        assert!(report.contains("scaffolded/stock_"), "{report}");
        assert!(!report.contains("scaffolded/catalog_"), "{report}");
    }
}
