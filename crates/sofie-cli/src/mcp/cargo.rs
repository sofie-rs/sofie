use std::path::Path;
use std::process::{Command, ExitStatus, Output, Stdio};

use sofie_model::App;

use super::ToolOutput;
use super::changes::changed_files;
use super::read::{app_crates, test_functions};
use crate::cargo;
use crate::child_process::{self, ProcessGroup};
use crate::gen_command::{GenError, io_error};

const PROGRESS: [&str; 11] = [
    "Compiling",
    "Checking",
    "Finished",
    "Running",
    "Doc-tests",
    "Blocking",
    "Fresh",
    "Building",
    "Locking",
    "Adding",
    "Executable",
];

#[derive(Clone, Copy)]
pub(super) enum CargoCommand {
    Build,
    Test,
}

impl CargoCommand {
    fn name(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Test => "test",
        }
    }

    fn rest(self) -> String {
        format!("cargo {} --offline", self.name())
    }
}

pub(super) fn run(root: &Path, command: CargoCommand, filter: Option<&str>) -> ToolOutput {
    match sofie_model::load(root)
        .map_err(GenError::Model)
        .and_then(|app| cargo(root, &app, command, filter))
    {
        Ok(output) => output,
        Err(error) => ToolOutput::from_error(root, &error),
    }
}

fn cargo(
    root: &Path,
    app: &App,
    command: CargoCommand,
    filter: Option<&str>,
) -> Result<ToolOutput, GenError> {
    let output = run_cargo(
        root,
        app_cargo(
            root,
            app,
            &[command.name(), "--offline", "--message-format", "short"],
        )
        .args(filter_arguments(filter)?),
    )?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let succeeded = output.status.success();
    let (summary, succeeded) = match command {
        CargoCommand::Build => (diagnostics(&stderr), succeeded),
        CargoCommand::Test => {
            // A failed build prints no test output and runs no test, whatever the filter.
            let filter = filter.filter(|_| succeeded || !stdout.trim().is_empty());
            let (files, unknown_changes) = test_files(root, app);
            let summary =
                test_summary(&stderr, &stdout, filter, &files).map(
                    |summary| match unknown_changes {
                        Some(error) => format!("{summary}\nchanged files unknown: {error}"),
                        None => summary,
                    },
                );
            match summary {
                Ok(summary) => (summary, succeeded),
                Err(unmatched) => (unmatched, false),
            }
        }
    };
    let verdict = verdict(&format!("cargo {}", command.name()), output.status);
    Ok(ToolOutput::judged(format!("{verdict}\n{summary}"), succeeded).with_rest(command.rest()))
}

/// After `--`, so the filter reaches the test binary and never cargo as an option.
fn filter_arguments(filter: Option<&str>) -> Result<Vec<&str>, GenError> {
    match filter {
        None => Ok(Vec::new()),
        Some(filter) if filter.starts_with('-') => Err(GenError::Usage(format!(
            "`{filter}` is no test name filter: a filter names tests and never starts with `-`"
        ))),
        Some(filter) => Ok(vec!["--", filter]),
    }
}

pub(super) fn lint(root: &Path, write: bool) -> ToolOutput {
    match sofie_model::load(root)
        .map_err(GenError::Model)
        .and_then(|app| lint_app(root, &app, write))
    {
        Ok(output) => output,
        Err(error) => ToolOutput::from_error(root, &error),
    }
}

/// Clippy is scoped to the app's packages, and `--no-deps` keeps it from linting the
/// workspace members they depend on, such as the framework's crates around an example.
fn lint_app(root: &Path, app: &App, write: bool) -> Result<ToolOutput, GenError> {
    let format_arguments: &[&str] = if write {
        &["fmt", "--message-format", "short"]
    } else {
        &["fmt", "--check", "--message-format", "short"]
    };
    let format = run_cargo(root, &mut app_cargo(root, app, format_arguments))?;
    let clippy = run_cargo(
        root,
        app_cargo(
            root,
            app,
            &[
                "clippy",
                "--offline",
                "--all-targets",
                "--no-deps",
                "--message-format",
                "short",
            ],
        )
        .args(["--", "-D", "warnings"]),
    )?;
    let clippy_report = format!(
        "{}\n{}",
        verdict("cargo clippy", clippy.status),
        diagnostics(&String::from_utf8_lossy(&clippy.stderr))
    );
    let text = format!("{}\n{clippy_report}", format_report(&format, write));
    Ok(ToolOutput::judged(
        text,
        format.status.success() && clippy.status.success(),
    ))
}

fn format_report(output: &Output, write: bool) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let files = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|file| format!("  {file}"));
    let mut lines: Vec<String> = files.collect();
    let header = match (lines.is_empty(), write) {
        (false, true) => "cargo fmt formatted these files:".to_owned(),
        (false, false) => {
            "cargo fmt --check: these files need formatting; `lint` with `write` formats them:"
                .to_owned()
        }
        (true, _) if output.status.success() => "cargo fmt: every file is formatted".to_owned(),
        (true, _) => verdict("cargo fmt", output.status),
    };
    lines.insert(0, header);
    let stderr = String::from_utf8_lossy(&output.stderr);
    lines.extend(diagnostic_lines(&stderr).map(str::to_owned));
    lines.join("\n")
}

fn app_cargo(root: &Path, app: &App, arguments: &[&str]) -> Command {
    let mut cargo = cargo::command();
    // Escape codes from a `CARGO_TERM_COLOR=always` environment would hide the progress lines.
    cargo
        .args(arguments)
        .current_dir(root)
        .stdin(Stdio::null())
        .env("CARGO_TERM_COLOR", "never");
    for package in packages(app) {
        cargo.args(["--package", package]);
    }
    cargo
}

fn run_cargo(root: &Path, cargo: &mut Command) -> Result<Output, GenError> {
    child_process::output(ProcessGroup::Own, cargo).map_err(|source| io_error(root, source))
}

fn verdict(command: &str, status: ExitStatus) -> String {
    match status.code() {
        Some(0) => format!("{command} succeeded"),
        Some(code) => format!("{command} failed with exit code {code}"),
        None => format!("{command} was stopped by a signal"),
    }
}

fn packages(app: &App) -> Vec<&str> {
    let mut packages = vec![app.app.package.as_str()];
    for module in &app.modules {
        packages.push(&module.krate.package);
        packages.extend(
            module
                .contract
                .iter()
                .map(|contract| contract.package.as_str()),
        );
    }
    packages
}

fn diagnostics(stderr: &str) -> String {
    diagnostic_lines(stderr).collect::<Vec<_>>().join("\n")
}

fn diagnostic_lines(stderr: &str) -> impl Iterator<Item = &str> {
    stderr.lines().filter(|line| {
        let first = line.split_whitespace().next().unwrap_or("");
        !line.trim().is_empty() && !PROGRESS.contains(&first)
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Failed,
    Ignored,
    Passed,
}

impl Outcome {
    const ALL: [Self; 3] = [Self::Passed, Self::Failed, Self::Ignored];

    fn name(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::Ignored => "ignored",
            Self::Passed => "passed",
        }
    }
}

/// `test <name> ... <outcome>`, as libtest prints each test it ran.
fn test_outcome(line: &str) -> Option<(&str, Outcome)> {
    let (name, result) = line.strip_prefix("test ")?.rsplit_once(" ... ")?;
    let outcome = match result {
        "ok" => Outcome::Passed,
        "FAILED" => Outcome::Failed,
        ignored if ignored.starts_with("ignored") => Outcome::Ignored,
        _ => return None,
    };
    Some((name, outcome))
}

fn is_progress(line: &str) -> bool {
    let ran_nothing =
        line.starts_with("test result:") && line.contains(" 0 passed; 0 failed; 0 ignored;");
    line.is_empty() || line.starts_with("running ") || ran_nothing
}

/// `Err` when a filter matched no test, which is the caller's mistake.
fn test_summary(
    stderr: &str,
    stdout: &str,
    filter: Option<&str>,
    files: &[TestFile],
) -> Result<String, String> {
    let ran: Vec<(&str, Outcome)> = stdout.lines().filter_map(test_outcome).collect();
    let details = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !is_progress(line) && test_outcome(line).is_none());
    let output = diagnostic_lines(stderr).chain(details).map(str::to_owned);
    if let Some(filter) = filter.filter(|_| ran.is_empty()) {
        let mut lines: Vec<String> = output.collect();
        lines.push(format!(
            "no test path contains `{filter}`, so none ran; a path starts within its test \
             binary, so a test file's name is no prefix of it"
        ));
        return Err(lines.join("\n"));
    }
    let mut lines = vec![counts(&ran)];
    lines.extend(output);
    let failed: Vec<&str> = named(&ran, |outcome| outcome == Outcome::Failed)
        .map(|(name, _)| name)
        .collect();
    if !failed.is_empty() {
        lines.push(format!("failed ({}):", failed.len()));
        lines.extend(failed.iter().map(|name| format!("  {name}")));
    }
    lines.extend(by_file(&ran, files, filter.is_some()));
    if let Some(filter) = filter {
        lines.extend(left_out(filter, files, &ran));
    }
    Ok(lines.join("\n"))
}

fn counts(ran: &[(&str, Outcome)]) -> String {
    let counted: Vec<String> = Outcome::ALL
        .iter()
        .map(|outcome| {
            let count = ran.iter().filter(|(_, ran)| ran == outcome).count();
            format!("{} {count}", outcome.name())
        })
        .collect();
    counted.join(", ")
}

fn named<'ran>(
    ran: &'ran [(&'ran str, Outcome)],
    keep: impl Fn(Outcome) -> bool,
) -> impl Iterator<Item = (&'ran str, Outcome)> {
    ran.iter()
        .copied()
        .filter(move |(_, outcome)| keep(*outcome))
}

/// Every name in changed files, or in every file under a filter; counts alone elsewhere,
/// so a large suite's answer stays short.
fn by_file(ran: &[(&str, Outcome)], files: &[TestFile], every_name: bool) -> Vec<String> {
    let mut unplaced: Vec<(&str, Outcome)> =
        named(ran, |outcome| outcome != Outcome::Failed).collect();
    if unplaced.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![if every_name {
        "passed and ignored, by test file:".to_owned()
    } else {
        "passed and ignored, by test file (every name in changed files):".to_owned()
    }];
    for file in files {
        let (placed, rest) = place(unplaced, file);
        unplaced = rest;
        let changed = if file.changed { " (changed)" } else { "" };
        lines.extend(file_lines(
            &format!("{}{changed}", file.file),
            &placed,
            every_name || file.changed,
        ));
    }
    lines.extend(file_lines("in no file of the app", &unplaced, every_name));
    lines
}

type Run<'ran> = (&'ran str, Outcome);

/// One run per test the file declares: libtest names a test by its path within its binary
/// alone, so two test binaries may each run a `works`.
fn place<'ran>(unplaced: Vec<Run<'ran>>, file: &TestFile) -> (Vec<Run<'ran>>, Vec<Run<'ran>>) {
    let mut taken = vec![false; unplaced.len()];
    for test in &file.tests {
        let first = unplaced
            .iter()
            .zip(&taken)
            .position(|((name, _), taken)| !taken && is_same_test(name, test));
        if let Some(at) = first {
            taken[at] = true;
        }
    }
    let (placed, rest): (Vec<_>, Vec<_>) = unplaced
        .into_iter()
        .zip(taken)
        .partition(|(_, taken)| *taken);
    let runs = |runs: Vec<(Run<'ran>, bool)>| runs.into_iter().map(|(run, _)| run).collect();
    (runs(placed), runs(rest))
}

fn file_lines(label: &str, tests: &[(&str, Outcome)], every_name: bool) -> Vec<String> {
    if tests.is_empty() {
        return Vec::new();
    }
    let ignored = tests
        .iter()
        .filter(|(_, outcome)| *outcome == Outcome::Ignored)
        .count();
    let passed = tests.len() - ignored;
    let tally = match ignored {
        0 => format!("  {label}: {passed} passed"),
        _ => format!("  {label}: {passed} passed, {ignored} ignored"),
    };
    let mut lines = vec![tally];
    if every_name {
        lines.extend(tests.iter().map(|(name, outcome)| match outcome {
            Outcome::Ignored => format!("    {name} (ignored)"),
            _ => format!("    {name}"),
        }));
    }
    lines
}

fn left_out(filter: &str, files: &[TestFile], ran: &[(&str, Outcome)]) -> Vec<String> {
    let changed = files.iter().filter(|file| file.changed);
    let total: usize = changed.clone().map(|file| file.tests.len()).sum();
    let skipped: Vec<String> = changed
        .flat_map(|file| {
            file.tests
                .iter()
                .filter(|test| !ran.iter().any(|(name, _)| is_same_test(name, test)))
                .map(move |test| format!("  {}: {test}", file.file))
        })
        .collect();
    if skipped.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "the filter `{filter}` left out {} of the {total} tests in changed files:",
        skipped.len()
    )];
    lines.extend(skipped);
    lines
}

/// `ran` is the path within the test binary; `path` is too where the file's place in its
/// binary is known, else within the file, which a module may nest.
fn is_same_test(ran: &str, path: &str) -> bool {
    ran == path || ran.ends_with(&format!("::{path}"))
}

struct TestFile {
    /// Relative to the app root.
    file: String,
    changed: bool,
    tests: Vec<String>,
}

/// The app's files with tests, changed files first; a file that does not parse shows in the
/// build's own errors. With why, when git cannot say which files changed.
fn test_files(root: &Path, app: &App) -> (Vec<TestFile>, Option<String>) {
    let (changed, unknown) = match changed_files(root) {
        Ok(changed) => (changed, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    let mut files: Vec<TestFile> = app_crates(app)
        .flat_map(|krate| {
            krate
                .sources
                .iter()
                .chain(&krate.tests_and_examples)
                .filter_map(|source| test_file(root, &krate.path, &source.file, &changed))
        })
        .collect();
    files.sort_by(|left, right| (!left.changed, &left.file).cmp(&(!right.changed, &right.file)));
    (files, unknown)
}

fn test_file(root: &Path, krate: &Path, file: &Path, changed: &[String]) -> Option<TestFile> {
    let prefix = file
        .strip_prefix(krate)
        .ok()
        .and_then(module_prefix)
        .unwrap_or_default();
    let tests: Vec<String> = test_functions(file)
        .ok()?
        .into_iter()
        .map(|test| format!("{prefix}{}", test.path))
        .collect();
    let relative = file.strip_prefix(root).unwrap_or(file);
    (!tests.is_empty()).then(|| TestFile {
        file: relative.display().to_string(),
        changed: changed.iter().any(|path| Path::new(path) == relative),
        tests,
    })
}

/// The module a test binary puts a file in, by Cargo's target layout: `src/orders.rs` is
/// `orders::` of the library, `tests/scaffolded/order.rs` `order::` of the `scaffolded` test.
fn module_prefix(within_crate: &Path) -> Option<String> {
    let mut parts: Vec<String> = within_crate
        .with_extension("")
        .iter()
        .map(|part| part.to_string_lossy().into_owned())
        .collect();
    match parts.first().map(String::as_str) {
        Some("src") => {
            parts.remove(0);
        }
        Some("tests" | "benches" | "examples") => {
            parts.drain(..2.min(parts.len()));
        }
        _ => return None,
    }
    if matches!(
        parts.last().map(String::as_str),
        Some("lib" | "main" | "mod")
    ) {
        parts.pop();
    }
    Some(
        parts
            .iter()
            .flat_map(|part| [part.as_str(), "::"])
            .collect(),
    )
}

pub(crate) fn build_failure(text: &str) -> String {
    diagnostics(text)
}

pub(super) fn build_rest() -> String {
    CargoCommand::Build.rest()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{TestFile, filter_arguments, module_prefix, test_summary};

    /// Before `--`, a filter such as `--config=build.rustc-wrapper=…` was an option of cargo itself.
    #[test]
    fn a_filter_reaches_the_test_binary_only_and_never_as_an_option() {
        assert_eq!(filter_arguments(Some("orders")).unwrap(), ["--", "orders"]);
        assert_eq!(filter_arguments(None).unwrap(), [] as [&str; 0]);
        assert!(filter_arguments(Some("--config=build.rustc-wrapper=/tmp/evil")).is_err());
    }

    fn file(file: &str, changed: bool, tests: &[String]) -> TestFile {
        TestFile {
            file: file.to_owned(),
            changed,
            tests: tests.to_vec(),
        }
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn run_of_63() -> (String, [TestFile; 2]) {
        let listed: Vec<String> = (0..60)
            .map(|number| format!("orders::tests::lists_{number}"))
            .collect();
        let ran: Vec<String> = listed
            .iter()
            .map(|name| format!("test {name} ... ok"))
            .collect();
        let stdout = format!(
            "running 63 tests\n{}\ntest order_detail::shows ... ok\n\
             test order_detail::links ... ok\ntest order_detail::slow ... ignored\n",
            ran.join("\n")
        );
        let changed = names(&[
            "order_detail::shows",
            "order_detail::links",
            "order_detail::slow",
            "order_detail::refuses",
        ]);
        let files = [
            file("app/tests/scaffolded/order_detail.rs", true, &changed),
            file("app/src/orders.rs", false, &listed),
        ];
        (stdout, files)
    }

    #[test]
    fn a_test_summary_counts_first_then_gives_each_failure_in_full() {
        let stderr = "   Compiling tiny v0.1.0\nsrc/lib.rs:2:5: warning: unused variable: `x`\n    \
                      Finished `test` profile\n     Running unittests src/lib.rs\n";
        let stdout = "\nrunning 3 tests\ntest adds ... ok\ntest slow ... ignored, takes a minute\n\
                      test subtracts ... FAILED\n\nfailures:\n\n---- subtracts stdout ----\n\
                      assertion failed\n\ntest result: FAILED. 1 passed; 1 failed; 1 ignored\n\n\
                      running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured\n";
        let nothing =
            "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured\n";

        let text = test_summary(stderr, stdout, None, &[]);
        let unmatched = test_summary("", nothing, Some("missing"), &[]);

        assert_eq!(
            text.as_deref(),
            Ok("passed 1, failed 1, ignored 1\n\
                src/lib.rs:2:5: warning: unused variable: `x`\nfailures:\n\
                ---- subtracts stdout ----\nassertion failed\n\
                test result: FAILED. 1 passed; 1 failed; 1 ignored\n\
                failed (1):\n  subtracts\n\
                passed and ignored, by test file (every name in changed files):\n  \
                in no file of the app: 1 passed, 1 ignored")
        );
        assert_eq!(
            unmatched,
            Err(
                "no test path contains `missing`, so none ran; a path starts within its test \
                 binary, so a test file's name is no prefix of it"
                    .to_owned()
            )
        );
    }

    /// A cut at 50 names once hid whether a test just written had run.
    #[test]
    fn a_full_run_names_every_test_of_changed_files_and_counts_the_others() {
        let (stdout, files) = run_of_63();

        let text = test_summary("", &stdout, None, &files).unwrap();

        assert!(
            text.starts_with("passed 62, failed 0, ignored 1\n"),
            "{text}"
        );
        assert!(
            text.ends_with(
                "  app/tests/scaffolded/order_detail.rs (changed): 2 passed, 1 ignored\n    \
                 order_detail::shows\n    order_detail::links\n    \
                 order_detail::slow (ignored)\n  app/src/orders.rs: 60 passed"
            ),
            "{text}"
        );
    }

    #[test]
    fn a_filter_names_every_test_it_ran_and_those_of_changed_files_it_left_out() {
        let (stdout, files) = run_of_63();

        let text = test_summary("", &stdout, Some("o"), &files).unwrap();

        assert!(
            text.contains("  app/src/orders.rs: 60 passed\n    orders::tests::lists_0\n"),
            "{text}"
        );
        assert!(text.contains("\n    orders::tests::lists_59\n"), "{text}");
        assert!(
            text.ends_with(
                "the filter `o` left out 1 of the 4 tests in changed files:\n  \
                 app/tests/scaffolded/order_detail.rs: order_detail::refuses"
            ),
            "{text}"
        );
    }

    #[test]
    fn a_test_path_two_binaries_share_is_credited_to_each_of_them() {
        let stdout = "running 1 test\ntest works ... ok\n\nrunning 1 test\ntest works ... ok\n";
        let files = [
            file("app/tests/first.rs", true, &names(&["works"])),
            file("app/tests/second.rs", true, &names(&["works"])),
        ];

        let text = test_summary("", stdout, None, &files).unwrap();

        assert!(
            text.ends_with(
                "  app/tests/first.rs (changed): 1 passed\n    works\n  \
                 app/tests/second.rs (changed): 1 passed\n    works"
            ),
            "{text}"
        );
    }

    #[test]
    fn a_file_s_place_in_its_test_binary_follows_cargo_s_layout() {
        let prefix = |path: &str| module_prefix(Path::new(path));

        assert_eq!(prefix("src/lib.rs").as_deref(), Some(""));
        assert_eq!(prefix("src/areas/mod.rs").as_deref(), Some("areas::"));
        assert_eq!(
            prefix("src/areas/customer/routes.rs").as_deref(),
            Some("areas::customer::routes::")
        );
        assert_eq!(prefix("tests/orders.rs").as_deref(), Some(""));
        assert_eq!(prefix("tests/scaffolded/main.rs").as_deref(), Some(""));
        assert_eq!(
            prefix("tests/scaffolded/order_detail.rs").as_deref(),
            Some("order_detail::")
        );
        assert_eq!(prefix("build.rs"), None);
    }
}
