use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use sofie_check::Finding;
use sofie_check::report;
use sofie_cli::{
    BenchReport, BuildOptions, Built, FeatureChange, FuzzOptions, GenError, GenMode, GenOutcome,
    Image, NEW_COMMANDS, NewCode, NewCommand, Verdict, bench, build, change_feature, check_app,
    check_config, check_schema, component_list, explain, fix_app, fresh_database, fuzz, new_app,
    new_code, new_module, regenerate, regenerated, route_table, run_schedule, scaffold_screen,
    scaffold_tests, seed, split_module, split_plan, task_table, upgrade,
};

const USAGE_BEFORE_NEW: &str = "usage: sofie gen [--check] [ROOT]\n       \
                     sofie gen migration MODULE NAME [ROOT]\n       sofie check [ROOT]\n       \
                     sofie dev [--at INSTANT] [--remote-database] [ROOT]\n       \
                     sofie build [--image | --worker] [--pgo] [ROOT]\n       \
                     sofie bench [--target URL] [--environment NAME] [ROOT]\n       \
                     sofie fuzz [--seed N] [--requests N] [ROOT]\n       \
                     sofie seed [--volume N] [ROOT]\n       \
                     sofie fix [ROOT]\n       \
                     sofie feature add|remove NAME [ROOT]\n       \
                     sofie upgrade [--write] [ROOT]\n       \
                     sofie config check --env ENVIRONMENT [ROOT]\n       sofie db fresh [ROOT]\n       \
                     sofie schedule run NAME [ROOT]\n       \
                     sofie schema check [ROOT]\n       \
                     sofie routes [ROOT]\n       sofie tasks [ROOT]\n       \
                     sofie components\n       \
                     sofie explain NAME [ROOT]\n       \
                     sofie new app NAME\n       \
                     sofie new module NAME [ROOT]";
const USAGE_AFTER_NEW: &str = "sofie scaffold screen KEY [ROOT]\n       \
                     sofie scaffold tests [ROOT]\n       \
                     sofie split MODULE [--write] [ROOT]\n       \
                     sofie docs [--check] [ROOT]   (feature `docs`)\n       \
                     sofie openapi [ROOT]          (feature `openapi`)\n       \
                     sofie typescript [ROOT]       (feature `typescript`)\n       \
                     sofie mcp [ROOT]              (feature `mcp`)";
const FINDINGS: u8 = 1;
const FAILED: u8 = 2;

enum Command {
    Gen(GenMode, PathBuf),
    Dev(DevOptions),
    Build(BuildOptions, PathBuf),
    Bench(BenchOptions),
    Fuzz(FuzzOptions, PathBuf),
    Seed(usize, PathBuf),
    Check(PathBuf),
    ConfigCheck(String, PathBuf),
    FreshDatabase(PathBuf),
    RunSchedule(String, PathBuf),
    SchemaCheck(PathBuf),
    Routes(PathBuf),
    Tasks(PathBuf),
    Components,
    Help,
    Explain(String, PathBuf),
    Fix(PathBuf),
    Feature(String, FeatureChange, PathBuf),
    Upgrade(bool, PathBuf),
    NewApp(String),
    NewModule(String, PathBuf),
    NewCode(NewCode, PathBuf),
    ScaffoldScreen(String, PathBuf),
    ScaffoldTests(PathBuf),
    Split(String, bool, PathBuf),
    #[cfg(feature = "docs")]
    Docs(GenMode, PathBuf),
    #[cfg(feature = "openapi")]
    OpenApi(PathBuf),
    #[cfg(feature = "typescript")]
    TypeScript(PathBuf),
    #[cfg(feature = "mcp")]
    Mcp(PathBuf),
    /// Arguments a command recognised but refused, with why.
    Refused(String),
}

fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().skip(1).collect();
    match parse(&arguments) {
        Some(Command::Gen(mode, root)) => report_outcomes(&root, "gen", regenerated(&root, mode)),
        Some(Command::Fix(root)) => run_fix(&root),
        Some(Command::Feature(name, change, root)) => run_feature(&root, &name, change),
        Some(Command::Upgrade(write, root)) => run_upgrade(&root, write),
        Some(Command::Dev(options)) => run_dev(&options),
        Some(Command::Build(options, root)) => run_build(&root, options),
        Some(Command::Bench(options)) => run_bench(&options),
        Some(Command::Fuzz(options, root)) => run_fuzz(&root, options),
        Some(Command::Seed(volume, root)) => run_seed(&root, volume),
        Some(Command::NewApp(name)) => {
            let code = report_outcomes(Path::new(&name), "gen", new_app(Path::new("."), &name));
            if code == ExitCode::SUCCESS {
                print_app_next_steps(&name);
            }
            code
        }
        Some(Command::NewModule(name, root)) => {
            report_outcomes(&root, "gen", new_module(&root, &name))
        }
        Some(Command::NewCode(code, root)) => run_new_code(&root, &code),
        Some(Command::ScaffoldScreen(key, root)) => run_scaffold_screen(&root, &key),
        Some(Command::ScaffoldTests(root)) => run_scaffold_tests(&root),
        Some(Command::Split(module, write, root)) => run_split(&root, &module, write),
        #[cfg(feature = "docs")]
        Some(Command::Docs(mode, root)) => {
            report_outcomes(&root, "docs", sofie_cli::write_docs(&root, mode))
        }
        #[cfg(feature = "openapi")]
        Some(Command::OpenApi(root)) => print_from_model(&root, |app| Ok(sofie_cli::openapi(app))),
        #[cfg(feature = "typescript")]
        Some(Command::TypeScript(root)) => {
            print_from_model(&root, |app| Ok(sofie_cli::typescript(app)))
        }
        #[cfg(feature = "mcp")]
        Some(Command::Mcp(root)) => run_mcp(&root),
        Some(Command::Check(root)) => report_findings(&root, check_app(&root)),
        Some(Command::ConfigCheck(environment, root)) => {
            report_findings(&root, check_config(&root, &environment))
        }
        Some(Command::FreshDatabase(root)) => run_fresh_database(&root),
        Some(Command::RunSchedule(name, root)) => match run_schedule(&root, &name) {
            Ok(line) => {
                println!("{line}");
                ExitCode::SUCCESS
            }
            Err(error) => failed(&root, &error),
        },
        Some(Command::SchemaCheck(root)) => run_schema_check(&root),
        Some(Command::Routes(root)) => print_from_model(&root, route_table),
        Some(Command::Tasks(root)) => print_from_model(&root, |app| Ok(task_table(app))),
        Some(Command::Components) => {
            print!("{}", component_list(None));
            ExitCode::SUCCESS
        }
        Some(Command::Explain(name, root)) => run_explain(&name, &root),
        Some(Command::Help) => {
            println!("{}", usage());
            ExitCode::SUCCESS
        }
        Some(Command::Refused(message)) => failed_with(&message),
        None => {
            eprintln!("{}", usage());
            ExitCode::from(FAILED)
        }
    }
}

fn usage() -> String {
    let lines: Vec<String> = std::iter::once(USAGE_BEFORE_NEW.to_owned())
        .chain(NEW_COMMANDS.iter().map(NewCommand::usage))
        .chain([USAGE_AFTER_NEW.to_owned()])
        .collect();
    lines.join("\n       ")
}

/// An MCP client reads stdout as the protocol, so the usage text would
/// show only as a closed connection.
#[cfg(not(all(
    feature = "docs",
    feature = "openapi",
    feature = "typescript",
    feature = "mcp"
)))]
fn left_out(feature: &str) -> Command {
    Command::Refused(format!(
        "`sofie {feature}` was left out of this build of sofie-cli; install it again with \
         `--features {feature}`, e.g. `cargo install --path crates/sofie-cli --features {feature}`"
    ))
}

fn parse(arguments: &[String]) -> Option<Command> {
    let (command, rest) = arguments.split_first()?;
    match (command.as_str(), rest) {
        ("gen", [flag, rest @ ..]) if flag == "--check" => {
            Some(Command::Gen(GenMode::Check, root(rest)?))
        }
        ("gen", [kind, module, name, rest @ ..])
            if kind == "migration" && !module.starts_with('-') && !name.starts_with('-') =>
        {
            let code = NewCode::SchemaMigration {
                module: module.clone(),
                name: name.clone(),
            };
            Some(Command::NewCode(code, root(rest)?))
        }
        ("gen", rest) => Some(Command::Gen(GenMode::Write, root(rest)?)),
        ("check", rest) => Some(Command::Check(root(rest)?)),
        ("dev", rest) => Some(Command::Dev(dev_options(rest)?)),
        ("build", rest) => build_options(rest),
        ("bench", rest) => Some(Command::Bench(bench_options(rest)?)),
        ("fuzz", rest) => fuzz_command(rest),
        ("seed", [flag, volume, rest @ ..]) if flag == "--volume" => {
            Some(Command::Seed(volume.parse().ok()?, root(rest)?))
        }
        ("seed", rest) => Some(Command::Seed(0, root(rest)?)),
        ("fix", rest) => Some(Command::Fix(root(rest)?)),
        ("feature", [action, name, rest @ ..]) if !name.starts_with('-') => {
            let change = match action.as_str() {
                "add" => FeatureChange::Add,
                "remove" => FeatureChange::Remove,
                _ => return None,
            };
            Some(Command::Feature(name.clone(), change, root(rest)?))
        }
        ("upgrade", [flag, rest @ ..]) if flag == "--write" => {
            Some(Command::Upgrade(true, root(rest)?))
        }
        ("upgrade", rest) => Some(Command::Upgrade(false, root(rest)?)),
        ("new", [kind, name]) if kind == "app" && !name.starts_with('-') => {
            Some(Command::NewApp(name.clone()))
        }
        ("new", [kind, name, rest @ ..]) if kind == "module" && !name.starts_with('-') => {
            Some(Command::NewModule(name.clone(), root(rest)?))
        }
        ("new", [kind, rest @ ..]) => new_code_command(kind, rest),
        ("scaffold", [kind, rest @ ..]) if kind == "tests" => {
            Some(Command::ScaffoldTests(root(rest)?))
        }
        ("scaffold", [kind, key, rest @ ..]) if kind == "screen" && !key.starts_with('-') => {
            Some(Command::ScaffoldScreen(key.clone(), root(rest)?))
        }
        ("split", [module, flag, rest @ ..]) if !module.starts_with('-') && flag == "--write" => {
            Some(Command::Split(module.clone(), true, root(rest)?))
        }
        ("split", [module, rest @ ..]) if !module.starts_with('-') => {
            Some(Command::Split(module.clone(), false, root(rest)?))
        }
        #[cfg(feature = "docs")]
        ("docs", [flag, rest @ ..]) if flag == "--check" => {
            Some(Command::Docs(GenMode::Check, root(rest)?))
        }
        #[cfg(feature = "docs")]
        ("docs", rest) => Some(Command::Docs(GenMode::Write, root(rest)?)),
        #[cfg(feature = "openapi")]
        ("openapi", rest) => Some(Command::OpenApi(root(rest)?)),
        #[cfg(feature = "typescript")]
        ("typescript", rest) => Some(Command::TypeScript(root(rest)?)),
        #[cfg(feature = "mcp")]
        ("mcp", rest) => Some(Command::Mcp(root(rest)?)),
        #[cfg(not(feature = "docs"))]
        ("docs", _) => Some(left_out("docs")),
        #[cfg(not(feature = "openapi"))]
        ("openapi", _) => Some(left_out("openapi")),
        #[cfg(not(feature = "typescript"))]
        ("typescript", _) => Some(left_out("typescript")),
        #[cfg(not(feature = "mcp"))]
        ("mcp", _) => Some(left_out("mcp")),
        ("config", [check, flag, environment, rest @ ..])
            if check == "check" && flag == "--env" && !environment.starts_with('-') =>
        {
            Some(Command::ConfigCheck(environment.clone(), root(rest)?))
        }
        ("db", [fresh, rest @ ..]) if fresh == "fresh" => Some(Command::FreshDatabase(root(rest)?)),
        ("schedule", [run, name, rest @ ..]) if run == "run" && !name.starts_with('-') => {
            Some(Command::RunSchedule(name.clone(), root(rest)?))
        }
        ("schema", [check, rest @ ..]) if check == "check" => {
            Some(Command::SchemaCheck(root(rest)?))
        }
        ("routes", rest) => Some(Command::Routes(root(rest)?)),
        ("tasks", rest) => Some(Command::Tasks(root(rest)?)),
        ("components", []) => Some(Command::Components),
        ("help" | "--help" | "-h", []) => Some(Command::Help),
        ("explain", [name, rest @ ..]) if !name.starts_with('-') => {
            Some(Command::Explain(name.clone(), root(rest)?))
        }
        _ => None,
    }
}

fn new_code_command(kind: &str, arguments: &[String]) -> Option<Command> {
    let command = NEW_COMMANDS.iter().find(|command| command.name == kind)?;
    match command.parse(arguments) {
        Ok((code, rest)) => Some(Command::NewCode(code, root(rest)?)),
        Err(message) => Some(Command::Refused(format!("sofie new {kind}: {message}"))),
    }
}

struct DevOptions {
    at: Option<String>,
    remote_database: bool,
    root: PathBuf,
}

fn dev_options(mut rest: &[String]) -> Option<DevOptions> {
    let (mut at, mut remote_database) = (None, false);
    loop {
        match rest {
            [flag, instant, remaining @ ..]
                if flag == "--at" && !instant.starts_with('-') && at.is_none() =>
            {
                at = Some(instant.clone());
                rest = remaining;
            }
            [flag, remaining @ ..] if flag == "--remote-database" && !remote_database => {
                remote_database = true;
                rest = remaining;
            }
            _ => break,
        }
    }
    Some(DevOptions {
        at,
        remote_database,
        root: root(rest)?,
    })
}

struct BenchOptions {
    target: Option<String>,
    environment: Option<String>,
    root: PathBuf,
}

fn bench_options(mut rest: &[String]) -> Option<BenchOptions> {
    let mut options = BenchOptions {
        target: None,
        environment: None,
        root: PathBuf::new(),
    };
    while let [flag, value, remaining @ ..] = rest {
        let slot = match flag.as_str() {
            "--target" => &mut options.target,
            "--environment" => &mut options.environment,
            _ => break,
        };
        if value.starts_with('-') || slot.replace(value.clone()).is_some() {
            return None;
        }
        rest = remaining;
    }
    options.root = root(rest)?;
    Some(options)
}

fn fuzz_command(mut rest: &[String]) -> Option<Command> {
    let (mut seed, mut requests) = (None, None);
    while let [flag, value, remaining @ ..] = rest {
        match flag.as_str() {
            "--seed" => seed = Some(value.parse().ok()?),
            "--requests" => requests = Some(value.parse().ok()?),
            _ => break,
        }
        rest = remaining;
    }
    Some(Command::Fuzz(FuzzOptions::new(seed, requests), root(rest)?))
}

fn build_options(mut rest: &[String]) -> Option<Command> {
    let mut options = BuildOptions::default();
    while let [flag, remaining @ ..] = rest {
        let slot = match flag.as_str() {
            "--image" => &mut options.image,
            "--worker" => &mut options.worker,
            "--pgo" => &mut options.pgo,
            _ => break,
        };
        if std::mem::replace(slot, true) {
            return None;
        }
        rest = remaining;
    }
    Some(Command::Build(options, root(rest)?))
}

fn root(rest: &[String]) -> Option<PathBuf> {
    match rest {
        [] => Some(PathBuf::from(".")),
        [root] if !root.starts_with('-') => Some(PathBuf::from(root)),
        _ => None,
    }
}

/// The app does not start until `.env` holds a database and signing keys, which `new`
/// leaves to the developer since both are secrets of their own machine.
fn print_app_next_steps(name: &str) {
    println!(
        "\nnext:\n  cd {name}\n  cp .env.example .env    # set DATABASE_URL, and \
         SOFIE_SIGNING_KEYS to `openssl rand -base64 32`\n  sofie dev"
    );
}

fn report_outcomes(
    root: &Path,
    command: &str,
    outcomes: Result<Vec<GenOutcome>, GenError>,
) -> ExitCode {
    let outcomes = match outcomes {
        Ok(outcomes) => outcomes,
        Err(error) => return failed(root, &error),
    };
    let mut stale = false;
    for outcome in outcomes {
        match outcome {
            GenOutcome::Written(path) => println!("wrote {}", path.display()),
            GenOutcome::Removed(path) => println!("removed {}", path.display()),
            GenOutcome::UpToDate(_) => {}
            GenOutcome::Stale(path) => {
                eprintln!(
                    "{}: stale; run `sofie {command} {}`",
                    path.display(),
                    root.display()
                );
                stale = true;
            }
        }
    }
    found(stale)
}

fn report_findings(root: &Path, findings: Result<Vec<Finding>, GenError>) -> ExitCode {
    match findings {
        Ok(findings) => {
            print!("{}", report(root, &findings));
            found(!findings.is_empty())
        }
        Err(error) => failed(root, &error),
    }
}

fn failed(root: &Path, error: &GenError) -> ExitCode {
    if let GenError::Check(findings) = error {
        eprint!("{}", report(root, findings));
        return ExitCode::from(FINDINGS);
    }
    eprintln!("{error}");
    ExitCode::from(FAILED)
}

fn failed_with(error: &dyn std::fmt::Display) -> ExitCode {
    eprintln!("{error}");
    ExitCode::from(FAILED)
}

fn found(findings: bool) -> ExitCode {
    if findings {
        ExitCode::from(FINDINGS)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_split(root: &Path, module: &str, write: bool) -> ExitCode {
    if !write {
        return match split_plan(root, module) {
            Ok(plan) => {
                print!("{plan}\nRun `sofie split {module} --write` to carry it out.\n");
                ExitCode::SUCCESS
            }
            Err(error) => failed(root, &error),
        };
    }
    match split_module(root, module) {
        Ok(split) => {
            print!("{}", split.plan);
            report_outcomes(&split.plan.to, "gen", Ok(split.generated))
        }
        Err(error) => failed(root, &error),
    }
}

fn run_dev(options: &DevOptions) -> ExitCode {
    let root = &options.root;
    match sofie_cli::dev(root, options.at.as_deref(), options.remote_database) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => failed(root, &error),
    }
}

fn run_build(root: &Path, options: BuildOptions) -> ExitCode {
    let image = match build(root, options) {
        Ok(Built::Binary(binary)) => {
            #[expect(clippy::cast_precision_loss, reason = "a size shown to one decimal")]
            let megabytes = binary.bytes as f64 / 1_000_000.0;
            println!("built {} ({megabytes:.1} MB)", binary.path.display());
            return ExitCode::SUCCESS;
        }
        Ok(Built::Image(image)) => image,
        Err(error) => return failed(root, &error),
    };
    match image {
        Image::Built { tag } => {
            println!("built image {tag}");
            ExitCode::SUCCESS
        }
        Image::Unpinned { dockerfile } => {
            eprintln!(
                "{}: pin the builder and runtime images' digests (see its first lines), then run `sofie build --image` again",
                dockerfile.display()
            );
            ExitCode::from(FINDINGS)
        }
        Image::NoDocker {
            dockerfile,
            command,
        } => {
            println!(
                "wrote {}; docker was not found, build the image with `{command}`",
                dockerfile.display()
            );
            ExitCode::SUCCESS
        }
    }
}

fn run_bench(options: &BenchOptions) -> ExitCode {
    let report = match bench(
        &options.root,
        options.target.as_deref(),
        options.environment.as_deref(),
    ) {
        Ok(report) => report,
        Err(error) => return failed(&options.root, &error),
    };
    print_bench(&report);
    if report.comparison.regressed() {
        eprintln!(
            "sofie bench: a regression beyond the margin; {} is unchanged",
            report.baseline.display()
        );
        return ExitCode::from(FINDINGS);
    }
    println!("updated {}", report.baseline.display());
    ExitCode::SUCCESS
}

fn print_bench(report: &BenchReport) {
    for skipped in &report.skipped {
        println!("not loaded: {skipped}");
    }
    for route in &report.comparison.routes {
        let ceiling = route.baseline.map_or_else(
            || "new".to_owned(),
            |baseline| {
                format!(
                    "ceiling {:.2} ms, {:.2}%",
                    baseline.p95_ms,
                    baseline.error_rate * 100.0
                )
            },
        );
        let verdict = match &route.verdict {
            Verdict::New => "baseline set".to_owned(),
            Verdict::Held => "held".to_owned(),
            Verdict::Lowered => "lowered".to_owned(),
            Verdict::Regressed(reasons) => format!("REGRESSED: {}", reasons.join("; ")),
        };
        println!(
            "{}: p95 {:.2} ms, errors {:.2}% ({ceiling}): {verdict}",
            route.endpoint,
            route.measured.p95_ms,
            route.measured.error_rate * 100.0
        );
    }
}

fn run_fuzz(root: &Path, options: FuzzOptions) -> ExitCode {
    match fuzz(root, options) {
        Ok(report) => {
            print!("{report}");
            found(!report.findings.is_empty())
        }
        Err(error) => failed(root, &error),
    }
}

fn run_seed(root: &Path, volume: usize) -> ExitCode {
    match seed(root, volume) {
        Ok(seeded) => {
            print!("{seeded}");
            found(seeded.routes.iter().any(|route| route.refused > 0))
        }
        Err(error) => failed(root, &error),
    }
}

fn run_fresh_database(root: &Path) -> ExitCode {
    match fresh_database(root) {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(FAILED),
        Err(error) => failed(root, &error),
    }
}

fn run_schema_check(root: &Path) -> ExitCode {
    match check_schema(root) {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(FINDINGS),
        Err(error) => failed(root, &error),
    }
}

fn run_explain(name: &str, root: &Path) -> ExitCode {
    let app = match sofie_model::load(root) {
        Ok(app) => app,
        Err(error) => return failed_with(&error),
    };
    let sections = explain(&app, name);
    if sections.is_empty() {
        eprintln!(
            "nothing in the app is named `{name}`; name a route (`order.member.list_orders`), \
             area, module, event, job, task (`order.reindex`), read model, aggregate, screen \
             (`order/member/orders`, `sofie_identity/web/sign_in`), component, flag \
             (`order.split_payments`) or handler (`order::queries::list_orders`)"
        );
        return ExitCode::from(FINDINGS);
    }
    println!("{}", sections.join("\n\n"));
    ExitCode::SUCCESS
}

fn run_fix(root: &Path) -> ExitCode {
    let fixed = match fix_app(root) {
        Ok(fixed) => fixed,
        Err(error) => return failed(root, &error),
    };
    for change in &fixed.applied {
        println!("{change}");
    }
    report_findings(root, Ok(fixed.remaining))
}

fn run_upgrade(root: &Path, write: bool) -> ExitCode {
    let report = match upgrade(root, write) {
        Ok(report) => report,
        Err(error) => return failed(root, &error),
    };
    for change in &report.changes {
        println!(
            "{}:{} {} ({})",
            change.file.display(),
            change.line,
            change.codemod,
            change.change
        );
        for line in change.before.lines() {
            println!("- {line}");
        }
        for line in change.after.lines() {
            println!("+ {line}");
        }
    }
    match report.remaining {
        Some(remaining) => report_findings(root, Ok(remaining)),
        None if report.changes.is_empty() => {
            println!("nothing to upgrade");
            ExitCode::SUCCESS
        }
        None if report.sources_pending => {
            println!(
                "run `sofie upgrade --write` to apply these; source changes are planned once the \
                 manifests are upgraded"
            );
            ExitCode::SUCCESS
        }
        None => {
            println!("run `sofie upgrade --write` to apply these");
            ExitCode::SUCCESS
        }
    }
}

fn run_scaffold_tests(root: &Path) -> ExitCode {
    match scaffold_tests(root) {
        Ok(scaffolded) => {
            print!("{scaffolded}");
            ExitCode::SUCCESS
        }
        Err(error) => failed(root, &error),
    }
}

fn run_new_code(root: &Path, code: &NewCode) -> ExitCode {
    let changes = match new_code(root, code) {
        Ok(changes) => changes,
        Err(error) => return failed(root, &error),
    };
    for change in &changes {
        println!("{change}");
    }
    match regenerate(root) {
        Ok(written) => written.iter().for_each(|line| println!("{line}")),
        Err(error) => {
            let code = failed(root, &error);
            eprintln!(
                "sofie: kept the new code above; fix what `sofie gen` reports and run it again"
            );
            return code;
        }
    }
    report_findings(root, check_app(root))
}

fn run_feature(root: &Path, name: &str, change: FeatureChange) -> ExitCode {
    match change_feature(root, name, change) {
        Ok(edit) => println!("{edit}"),
        Err(error) => return failed(root, &error),
    }
    match regenerate(root) {
        Ok(written) => written.iter().for_each(|line| println!("{line}")),
        Err(error) => return failed(root, &error),
    }
    report_findings(root, check_app(root))
}

fn run_scaffold_screen(root: &Path, key: &str) -> ExitCode {
    match scaffold_screen(root, key) {
        Ok(file) => {
            println!("wrote {}", file.display());
            ExitCode::SUCCESS
        }
        Err(error) => failed(root, &error),
    }
}

fn print_from_model(
    root: &Path,
    render: fn(&sofie_model::App) -> Result<String, sofie_model::ModelError>,
) -> ExitCode {
    match sofie_model::load(root).and_then(|app| render(&app)) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(error) => failed_with(&error),
    }
}

#[cfg(feature = "mcp")]
fn run_mcp(root: &Path) -> ExitCode {
    match sofie_cli::serve_mcp(root, std::io::stdin().lock(), std::io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => failed_with(&format!("sofie mcp: {error}")),
    }
}
