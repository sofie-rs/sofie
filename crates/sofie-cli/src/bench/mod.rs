mod baseline;
mod journeys;
mod script;

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;
use sofie_model::App;

pub use baseline::{Comparison, Measurement, RouteResult, Verdict};

use baseline::compare;

use crate::build_command::{BuildOptions, build_binary};
use crate::dev_command::app_command;
use crate::gen_command::{GenError, io_error, read_existing};
use crate::local_app::{
    MEMBER_PASSWORD, bench_member, free_address, private_directory, refuse_remote_database,
    wait_answering,
};
use crate::source_edits::{read, write};
use script::bench_script;

const BENCHABLE: [&str; 3] = ["local", "testing", "staging"];
const STOP_WAIT: Duration = Duration::from_secs(30);
const LOG_TAIL: usize = 40;

pub struct BenchReport {
    pub skipped: Vec<String>,
    pub comparison: Comparison,
    pub baseline: PathBuf,
}

/// Without a target it builds the app, starts it on a free local port with
/// the shield off and its database on this machine, and loads that; a target
/// must run with `SOFIE_SHIELD=off`, and one not on this machine is signed in
/// to only as accounts named in `SOFIE_BENCH_<AREA>_EMAIL`/`_PASSWORD`.
pub fn bench(
    root: &Path,
    target: Option<&str>,
    environment: Option<&str>,
) -> Result<BenchReport, GenError> {
    let variable = std::env::var("SOFIE_ENVIRONMENT").ok();
    let environment = bench_environment(target, environment, variable.as_deref())?;
    if target.is_none() {
        refuse_remote_database(root, "bench")?;
    }
    let app = sofie_model::load(root).map_err(GenError::Model)?;
    let scripts = Scripts::write(root, &app)?;
    let measured = if let Some(target) = target {
        scripts.run(target)?
    } else {
        let built = build_binary(root, &app, BuildOptions::default())?;
        let running = serve(root, &built.path, &environment, &[])?;
        scripts.run(&running.target)?
    };
    let baseline_file = root.join("bench").join("baseline.json");
    let baseline = match read_existing(&baseline_file)? {
        Some(text) => baseline::parse(&text)
            .map_err(|error| GenError::App(format!("{}: {error}", baseline_file.display())))?,
        None => baseline::Measurements::new(),
    };
    let comparison = compare(&baseline, &measured);
    if !comparison.regressed() {
        write(&baseline_file, baseline::render(&comparison.next_baseline))?;
    }
    Ok(BenchReport {
        skipped: scripts.skipped,
        comparison,
        baseline: baseline_file,
    })
}

/// The generated k6 scripts under `bench/gen/`.
pub(crate) struct Scripts {
    endpoints: PathBuf,
    journeys: Option<PathBuf>,
    /// Each route left out, with its reason.
    skipped: Vec<String>,
    member_areas: BTreeSet<String>,
}

impl Scripts {
    pub(crate) fn write(root: &Path, app: &App) -> Result<Self, GenError> {
        let folder = root.join("bench").join("gen");
        let script = bench_script(app)?;
        let endpoints = folder.join("endpoints.js");
        write(&endpoints, &script.text)?;
        let client = journeys::client(app)?;
        write(&folder.join("client.ts"), &client.text)?;
        let mut member_areas: BTreeSet<String> = script.member_areas.into_iter().collect();
        let found = journeys::journeys(app)?;
        let runner = folder.join("journeys.ts");
        let journeys = if found.is_empty() {
            remove_stale(&runner)?;
            None
        } else {
            write(&runner, journeys::runner(&found))?;
            member_areas.extend(client.member_areas);
            Some(runner)
        };
        Ok(Self {
            endpoints,
            journeys,
            skipped: script.skipped,
            member_areas,
        })
    }

    pub(crate) fn run(&self, target: &str) -> Result<baseline::Measurements, GenError> {
        let accounts = accounts(target, &self.member_areas, |name| std::env::var(name).ok())?;
        let mut measured = run_k6(&self.endpoints, target, &accounts)?;
        if let Some(journeys) = &self.journeys {
            measured.extend(run_k6(journeys, target, &accounts)?);
        }
        Ok(measured)
    }
}

/// The account per member area as JSON for k6's environment, so no password
/// lands in a script or an argument list. A server not on this machine is
/// never signed up on: a known password would leave an account anyone can use.
fn accounts(
    target: &str,
    areas: &BTreeSet<String>,
    variable: impl Fn(&str) -> Option<String>,
) -> Result<String, GenError> {
    let local = is_local_target(target);
    let mut accounts = serde_json::Map::new();
    let mut missing = Vec::new();
    for area in areas {
        let account = if local {
            json!({ "email": bench_member(area), "password": MEMBER_PASSWORD, "signUp": true })
        } else {
            let prefix = format!("SOFIE_BENCH_{}", area.to_ascii_uppercase());
            let (email, password) = (format!("{prefix}_EMAIL"), format!("{prefix}_PASSWORD"));
            let (Some(email_value), Some(password_value)) = (variable(&email), variable(&password))
            else {
                missing.push(format!("{email} and {password} for area `{area}`"));
                continue;
            };
            json!({ "email": email_value, "password": password_value, "signUp": false })
        };
        accounts.insert(area.clone(), account);
    }
    if !missing.is_empty() {
        return Err(GenError::Usage(format!(
            "sofie bench signs up nobody on {target}, a server not on this machine; \
             name an existing account with {}",
            missing.join("; ")
        )));
    }
    Ok(serde_json::Value::Object(accounts).to_string())
}

fn is_local_target(target: &str) -> bool {
    let rest = target.split_once("://").map_or(target, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_and_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = match host_and_port.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => host_and_port.split(':').next().unwrap_or_default(),
    };
    matches!(
        host.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "::1"
    )
}

fn remove_stale(path: &Path) -> Result<(), GenError> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(io_error(path, error)),
        _ => Ok(()),
    }
}

/// Load against production would be an outage of our own making.
pub fn bench_environment(
    target: Option<&str>,
    flag: Option<&str>,
    variable: Option<&str>,
) -> Result<String, GenError> {
    let environment = match (flag.or(variable), target) {
        (Some(named), _) => named,
        (None, None) => "local",
        (None, Some(target)) => {
            return Err(GenError::Usage(format!(
                "name the environment {target} runs in with `--environment` or SOFIE_ENVIRONMENT; \
                 sofie bench loads only {}",
                BENCHABLE.join(", ")
            )));
        }
    };
    if let Some(target) = target.filter(|target| environment == "local" && !is_local_target(target))
    {
        return Err(GenError::Usage(format!(
            "{target} is not local, yet its environment is named `local`; name the environment \
             it runs in with `--environment`"
        )));
    }
    if !BENCHABLE.contains(&environment) {
        return Err(GenError::Usage(format!(
            "sofie bench refuses the `{environment}` environment; it loads only {}",
            BENCHABLE.join(", ")
        )));
    }
    Ok(environment.to_owned())
}

pub(crate) struct Running {
    child: Child,
    pub(crate) target: String,
    /// Holds the app's log, removed with it.
    directory: PathBuf,
}

impl Running {
    /// Ends it as an orchestrator would, so what the process writes at exit is written.
    pub(crate) fn stop(mut self) -> Result<(), GenError> {
        let pid = self.child.id().to_string();
        let sent = Command::new("kill")
            .args(["-TERM", &pid])
            .status()
            .map_err(|source| GenError::App(format!("sending SIGTERM to {pid}: {source}")))?;
        if !sent.success() {
            return Err(GenError::App(format!(
                "`kill -TERM {pid}` failed with {sent}"
            )));
        }
        let deadline = Instant::now() + STOP_WAIT;
        while Instant::now() < deadline {
            let exited = self
                .child
                .try_wait()
                .map_err(|source| GenError::App(format!("waiting for {pid}: {source}")))?;
            match exited {
                Some(status) if status.success() => return Ok(()),
                Some(status) => {
                    return Err(GenError::App(format!(
                        "the app (pid {pid}) ended with {status} after SIGTERM"
                    )));
                }
                None => {}
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err(GenError::App(format!(
            "the app (pid {pid}) did not stop within {} s of SIGTERM",
            STOP_WAIT.as_secs()
        )))
    }
}

/// Killed rather than stopped: the measurement is over, and nothing it holds
/// needs a graceful end.
impl Drop for Running {
    fn drop(&mut self) {
        if let Err(error) = self.child.kill().and_then(|()| self.child.wait().map(drop)) {
            eprintln!(
                "sofie bench: stopping the app (pid {}): {error}",
                self.child.id()
            );
        }
        if let Err(error) = fs::remove_dir_all(&self.directory) {
            eprintln!(
                "sofie bench: removing {}: {error}",
                self.directory.display()
            );
        }
    }
}

pub(crate) fn serve(
    root: &Path,
    binary: &Path,
    environment: &str,
    variables: &[(&str, &OsStr)],
) -> Result<Running, GenError> {
    let address = free_address()?;
    let directory = private_directory("sofie-bench")?;
    let log = directory.join("app.log");
    let output = File::create(&log).map_err(|source| io_error(&log, source))?;
    let errors = output
        .try_clone()
        .map_err(|source| io_error(&log, source))?;
    let child = app_command(root, binary)?
        .arg("serve")
        .envs(variables.iter().copied())
        .env("SOFIE_ADDRESS", address.to_string())
        .env("SOFIE_ENVIRONMENT", environment)
        .env("SOFIE_MIGRATE", "on")
        .env("SOFIE_SHIELD", "off")
        .stdout(output)
        .stderr(errors)
        .spawn()
        .map_err(|source| io_error(binary, source))?;
    let mut running = Running {
        child,
        target: format!("http://{address}"),
        directory,
    };
    let child = &mut running.child;
    if let Err(error) = wait_answering(address, "/ready", || {
        child.try_wait().map_err(|source| io_error(binary, source))
    }) {
        let text = read(&log)?;
        let lines: Vec<&str> = text.lines().collect();
        let last = lines[lines.len().saturating_sub(LOG_TAIL)..].join("\n");
        return Err(GenError::App(format!("{error}; its last lines:\n{last}")));
    }
    Ok(running)
}

fn run_k6(script: &Path, target: &str, accounts: &str) -> Result<baseline::Measurements, GenError> {
    let directory = private_directory("sofie-bench-summary")?;
    let measured = run_k6_into(script, target, accounts, &directory.join("summary.json"));
    fs::remove_dir_all(&directory).map_err(|source| io_error(&directory, source))?;
    measured
}

fn run_k6_into(
    script: &Path,
    target: &str,
    accounts: &str,
    summary: &Path,
) -> Result<baseline::Measurements, GenError> {
    let status = Command::new("k6")
        .arg("run")
        .arg("--env")
        .arg(format!(
            "SOFIE_BENCH_TARGET={}",
            target.trim_end_matches('/')
        ))
        .arg("--env")
        .arg(format!("SOFIE_BENCH_SUMMARY={}", summary.display()))
        .arg(script)
        .env("SOFIE_BENCH_ACCOUNTS", accounts)
        .stdin(Stdio::null())
        .status()
        .map_err(|source| {
            GenError::Usage(format!("running k6: {source}; install k6 (https://k6.io)"))
        })?;
    if !status.success() {
        return Err(GenError::App(format!("k6 failed with {status}")));
    }
    let text = read(summary)?;
    baseline::parse(&text)
        .map_err(|error| GenError::App(format!("k6's summary {}: {error}", summary.display())))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{accounts, bench_environment};

    #[test]
    fn signs_up_only_on_this_machine_and_takes_named_accounts_elsewhere() {
        let areas = BTreeSet::from(["customer".to_owned()]);
        let none = |_: &str| None;
        let named = |name: &str| Some(format!("{name}-value"));

        let local = accounts("http://127.0.0.1:4000", &areas, none).unwrap();
        let refused = accounts("https://staging.example.com", &areas, none).unwrap_err();
        let remote = accounts("https://staging.example.com", &areas, named).unwrap();

        assert!(local.contains("\"signUp\":true"), "{local}");
        assert!(local.contains("bench-customer@example.com"), "{local}");
        assert!(
            refused
                .to_string()
                .contains("SOFIE_BENCH_CUSTOMER_EMAIL and SOFIE_BENCH_CUSTOMER_PASSWORD"),
            "{refused}"
        );
        assert!(remote.contains("\"signUp\":false"), "{remote}");
        assert!(
            remote.contains("SOFIE_BENCH_CUSTOMER_PASSWORD-value"),
            "{remote}"
        );
        assert!(
            !remote.contains(crate::local_app::MEMBER_PASSWORD),
            "{remote}"
        );
    }

    #[test]
    fn a_target_is_local_only_on_a_loopback_host() {
        assert!(super::is_local_target("http://localhost:3000/"));
        assert!(super::is_local_target("http://[::1]:3000"));
        assert!(!super::is_local_target("https://localhost.example.com"));
        assert!(!super::is_local_target(
            "https://127.0.0.1@shop.example.com"
        ));
    }

    #[test]
    fn refuses_production_and_a_target_whose_environment_is_unnamed() {
        let production = bench_environment(None, Some("production"), None).unwrap_err();
        let unnamed = bench_environment(Some("https://shop.example.com"), None, None).unwrap_err();
        let from_variable = bench_environment(Some("https://shop.example.com"), None, Some("prod"));

        assert!(
            production
                .to_string()
                .contains("refuses the `production` environment")
        );
        assert!(
            unnamed
                .to_string()
                .contains("name the environment https://shop.example.com")
        );
        assert!(from_variable.is_err());
    }

    /// A developer's shell often exports `SOFIE_ENVIRONMENT=local` for local work.
    #[test]
    fn refuses_calling_a_remote_target_local() {
        let remote = "https://shop.example.com";

        let from_variable = bench_environment(Some(remote), None, Some("local")).unwrap_err();
        let from_flag = bench_environment(Some(remote), Some("local"), None);
        let here = bench_environment(Some("http://127.0.0.1:8080"), None, Some("local"));

        assert!(
            from_variable.to_string().contains("is not local"),
            "{from_variable}"
        );
        assert!(from_flag.is_err());
        assert_eq!(here.unwrap(), "local");
    }

    #[test]
    fn accepts_local_testing_and_staging_with_the_flag_over_the_variable() {
        assert_eq!(bench_environment(None, None, None).unwrap(), "local");
        assert_eq!(
            bench_environment(
                Some("https://staging.example.com"),
                Some("staging"),
                Some("production")
            )
            .unwrap(),
            "staging"
        );
        assert_eq!(
            bench_environment(None, None, Some("testing")).unwrap(),
            "testing"
        );
    }
}
