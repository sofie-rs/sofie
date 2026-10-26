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
use crate::scaffold_tests::{scaffold_tests_at, without_moment};

pub(super) enum Generator {
    NewModule(String),
    New(NewCode),
    ScaffoldScreen(String),
    EditScreen(ScreenEdit),
    /// The moment names the file it writes; `all` lists older gaps too.
    ScaffoldTests {
        now: Timestamp,
        all: bool,
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
    // `new_module` ends in its own `sofie gen`, already in its report.
    if !matches!(generator, Generator::NewModule(_)) {
        match regenerate(root) {
            Ok(written) if written.is_empty() => {
                text.push_str("sofie gen: everything up to date\n");
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
        Generator::ScaffoldTests { now, all } => {
            let scaffolded = scaffold_tests_at(root, *now)?;
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

fn outcome_lines(outcomes: &[GenOutcome]) -> String {
    let lines: Vec<String> = outcomes
        .iter()
        .filter_map(GenOutcome::change)
        .map(|change| format!("  {change}"))
        .collect();
    if lines.is_empty() {
        return "  everything up to date".to_owned();
    }
    lines.join("\n")
}
