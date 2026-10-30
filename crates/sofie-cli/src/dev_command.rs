use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use sofie_check::report;
use sofie_model::App;

use crate::cargo::{self, Diagnostics};
use crate::child_process::{ChildProcess, ProcessGroup, stop_on_interrupt};
use crate::clients::build_clients;
use crate::gen_command::{GenError, GenMode, generated_app, io_error};
use crate::islands::{IslandMode, build_islands};
use crate::local_app::{database_urls, remote_database_key};
use crate::log_buffer::LogBuffer;

const POLL: Duration = Duration::from_millis(300);
pub(crate) const DEVELOPMENT: &[&str] = &["--features", "sofie/development"];
pub(crate) const NOT_SOURCES: [&str; 3] = ["target", "node_modules", ".git"];

type Snapshot = BTreeMap<PathBuf, SystemTime>;

#[derive(Debug, PartialEq, Eq)]
enum Change {
    Templates,
    Islands,
    Code,
}

/// Development builds migrate on start, so a database on another host takes
/// `remote_database`, never what the shell or `.env` happens to hold.
pub fn dev(root: &Path, at: Option<&str>, remote_database: bool) -> Result<(), GenError> {
    if !remote_database {
        refuse_remote_development(&database_urls(root)?)?;
    }
    stop_on_interrupt().map_err(|source| io_error(root, source))?;
    let mut app = DevApp::new(root, at, AppOutput::Terminal);
    report_started(app.rebuild().map(Some));
    loop {
        std::thread::sleep(POLL);
        if let Some(status) = app.exited()? {
            eprintln!("sofie dev: the app exited with {status}; waiting for a change");
        }
        report_started(app.refresh());
    }
}

fn refuse_remote_development(urls: &BTreeMap<String, String>) -> Result<(), GenError> {
    match remote_database_key(urls) {
        Some(key) => Err(GenError::Usage(format!(
            "`sofie dev` migrates and writes to the app's database, and {key} names a host other \
             than this machine; pass `--remote-database` to run against it anyway"
        ))),
        None => Ok(()),
    }
}

fn report_started(started: Result<Option<u32>, GenError>) {
    match started {
        Ok(Some(pid)) => println!("sofie dev: running the app (pid {pid})"),
        Ok(None) => {}
        Err(error) => eprintln!("sofie dev: {error}\nsofie dev: waiting for a change"),
    }
}

pub(crate) enum AppOutput {
    Terminal,
    Captured {
        log: Arc<LogBuffer>,
        address: std::net::SocketAddr,
    },
}

impl AppOutput {
    fn spawn(&self, command: &mut Command) -> std::io::Result<Child> {
        match self {
            Self::Terminal => command.spawn(),
            Self::Captured { log, address } => {
                let mut child = command
                    .env("SOFIE_ADDRESS", address.to_string())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()?;
                log.follow(child.stdout.take(), child.stderr.take());
                Ok(child)
            }
        }
    }
}

/// The development app `sofie dev` and `sofie mcp` run: built with
/// `sofie/development`, restarted on a code change, signalled on a template or
/// island change, its client bundles rebuilt on a template change.
pub(crate) struct DevApp {
    root: PathBuf,
    at: Option<String>,
    output: AppOutput,
    variables: Vec<(&'static str, &'static str)>,
    process: Option<ChildProcess>,
    /// The running app's `SOFIE_DEV_DIR`.
    directory: Option<PathBuf>,
    generation: u64,
    built: Snapshot,
    build_failed: bool,
    last_exit: Option<ExitStatus>,
}

impl DevApp {
    pub(crate) fn new(root: &Path, at: Option<&str>, output: AppOutput) -> Self {
        Self {
            root: root.to_path_buf(),
            at: at.map(str::to_owned),
            output,
            variables: Vec::new(),
            process: None,
            directory: None,
            generation: 0,
            built: Snapshot::new(),
            build_failed: false,
            last_exit: None,
        }
    }

    pub(crate) fn set_variable(&mut self, key: &'static str, value: &'static str) {
        self.variables.push((key, value));
    }

    #[cfg(feature = "mcp")]
    pub(crate) fn pid(&self) -> Option<u32> {
        self.process.as_ref().map(ChildProcess::pid)
    }

    #[cfg(feature = "mcp")]
    pub(crate) fn build_failed(&self) -> bool {
        self.build_failed
    }

    #[cfg(feature = "mcp")]
    pub(crate) fn last_exit(&self) -> Option<ExitStatus> {
        self.last_exit
    }

    /// A failed build keeps the running app.
    pub(crate) fn rebuild(&mut self) -> Result<u32, GenError> {
        let generated = self.generate();
        // Taken before cargo runs: what gen wrote is not a change of its own,
        // but a file saved during the build is.
        self.built = snapshot(&self.root)?;
        let built = generated.and_then(|app| Ok((app, self.compile()?)));
        self.build_failed = built.is_err();
        let (app, executable) = built?;
        self.restart(&app, &executable)
    }

    /// The pid when it restarted.
    pub(crate) fn refresh(&mut self) -> Result<Option<u32>, GenError> {
        let current = snapshot(&self.root)?;
        let signalled = match classify(&self.built, &current) {
            None => return Ok(None),
            // After a failed build any edit, a template too, may be the fix.
            Some(_) if self.build_failed => return self.rebuild().map(Some),
            Some(Change::Templates) => self.rebuild_clients(),
            Some(Change::Islands) if self.process.is_some() => self.rebuild_islands(),
            Some(Change::Islands | Change::Code) => return self.rebuild().map(Some),
        };
        self.built = current;
        signalled.map(|()| None)
    }

    pub(crate) fn exited(&mut self) -> Result<Option<ExitStatus>, GenError> {
        let Some(process) = self.process.as_mut() else {
            return Ok(None);
        };
        let exited = process.exited().map_err(|source| {
            GenError::App(format!(
                "watching the app (pid {}) failed: {source}",
                process.pid()
            ))
        })?;
        let Some(status) = exited else {
            return Ok(None);
        };
        self.process = None;
        self.last_exit = Some(status);
        Ok(Some(status))
    }

    pub(crate) fn stop(&mut self) {
        if let Some(status) = self.process.take().and_then(ChildProcess::kill) {
            self.last_exit = Some(status);
        }
    }

    fn generate(&self) -> Result<App, GenError> {
        match generated_app(&self.root, GenMode::Write) {
            Err(GenError::Check(findings)) => Err(GenError::App(report(&self.root, &findings))),
            result => result.map(|(app, _)| app),
        }
    }

    fn diagnostics(&self) -> Diagnostics {
        match self.output {
            AppOutput::Terminal => Diagnostics::Terminal,
            #[cfg(feature = "mcp")]
            AppOutput::Captured { .. } => Diagnostics::Captured,
            #[cfg(not(feature = "mcp"))]
            AppOutput::Captured { .. } => Diagnostics::Terminal,
        }
    }

    fn compile(&self) -> Result<PathBuf, GenError> {
        cargo::build(&self.root, DEVELOPMENT, self.diagnostics())
    }

    fn restart(&mut self, app: &App, executable: &Path) -> Result<u32, GenError> {
        let directory = executable.parent().unwrap_or(&self.root).join("sofie-dev");
        build_islands(
            app,
            &directory.join("islands"),
            IslandMode::Development,
            self.diagnostics(),
        )?;
        build_development_clients(app, &directory, self.diagnostics())?;
        self.stop();
        let mut command = app_command(&self.root, executable)?;
        command
            .arg("serve")
            .args(self.at.iter().flat_map(|at| ["--at", at.as_str()]))
            .env("SOFIE_MIGRATE", "on")
            .env("SOFIE_DEV_DIR", &directory)
            .envs(self.variables.iter().copied());
        let process = ChildProcess::spawn(ProcessGroup::Inherited, &mut command, |command| {
            self.output.spawn(command)
        })
        .map_err(|source| io_error(executable, source))?;
        let pid = process.pid();
        self.process = Some(process);
        self.directory = Some(directory);
        Ok(pid)
    }

    fn rebuild_islands(&mut self) -> Result<(), GenError> {
        let Some(directory) = self.running_directory() else {
            return Ok(());
        };
        let app = sofie_model::load(&self.root).map_err(GenError::Model)?;
        build_islands(
            &app,
            &directory.join("islands"),
            IslandMode::Development,
            self.diagnostics(),
        )?;
        self.signal("islands")
    }

    /// A client area's screens are compiled into its bundle, so a page reload alone would show
    /// the old ones.
    fn rebuild_clients(&mut self) -> Result<(), GenError> {
        if let Some(directory) = self.running_directory() {
            let app = sofie_model::load(&self.root).map_err(GenError::Model)?;
            build_development_clients(&app, directory, self.diagnostics())?;
        }
        self.signal("page")
    }

    fn signal(&mut self, kind: &str) -> Result<(), GenError> {
        let Some(file) = self
            .running_directory()
            .map(|directory| directory.join("generation"))
        else {
            return Ok(());
        };
        self.generation += 1;
        fs::write(&file, format!("{} {kind}\n", self.generation))
            .map_err(|source| io_error(&file, source))
    }

    fn running_directory(&self) -> Option<&Path> {
        self.process.as_ref().and(self.directory.as_deref())
    }
}

/// Served instead of the embedded bundles while the app runs under `sofie dev`.
fn build_development_clients(
    app: &App,
    directory: &Path,
    diagnostics: Diagnostics,
) -> Result<(), GenError> {
    let screens = sofie_check::Screens::read(&app.modules);
    build_clients(
        app,
        &screens,
        &directory.join("clients"),
        IslandMode::Development,
        diagnostics,
    )
    .map(drop)
}

/// The development binary run once, as `sofie dev` runs it.
pub(crate) fn run_development(root: &Path, arguments: &[&str]) -> Result<ExitStatus, GenError> {
    let executable = cargo::build(root, DEVELOPMENT, Diagnostics::Terminal)?;
    app_command(root, &executable)?
        .args(arguments)
        .status()
        .map_err(|source| io_error(&executable, source))
}

/// The app's binary in the app root with its `.env`, as every command runs it.
pub(crate) fn app_command(root: &Path, executable: &Path) -> Result<Command, GenError> {
    let mut command = Command::new(executable);
    command
        .envs(dotenv(root)?)
        .current_dir(root)
        .stdin(Stdio::null());
    Ok(command)
}

/// The app root's `.env`, without the keys the environment sets.
pub(crate) fn dotenv(root: &Path) -> Result<Vec<(String, String)>, GenError> {
    let file = root.join(".env");
    let text = match fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(io_error(&file, source)),
    };
    Ok(env_entries(&text)
        .filter(|(_, key, _)| std::env::var_os(key).is_none())
        .map(|(_, key, value)| (key.to_owned(), value.to_owned()))
        .collect())
}

/// An env file's line numbers, keys and unquoted values.
pub(crate) fn env_entries(text: &str) -> impl Iterator<Item = (usize, &str, &str)> {
    text.lines().enumerate().filter_map(|(index, line)| {
        let line = line.trim();
        let (key, value) = line
            .strip_prefix("export ")
            .unwrap_or(line)
            .split_once('=')
            .filter(|_| !line.starts_with('#'))?;
        Some((index + 1, key.trim(), value_of(value.trim())))
    })
}

/// A quoted value ends at its closing quote; an unquoted one at a ` #` comment.
fn value_of(written: &str) -> &str {
    if let Some(quote) = written
        .chars()
        .next()
        .filter(|first| matches!(first, '"' | '\''))
    {
        return written[1..]
            .find(quote)
            .map_or(written, |end| &written[1..=end]);
    }
    written
        .char_indices()
        .find(|&(at, character)| {
            character.is_whitespace() && written[at..].trim_start().starts_with('#')
        })
        .map_or(written, |(at, _)| &written[..at])
}

fn snapshot(root: &Path) -> Result<Snapshot, GenError> {
    let mut walk = Walk::default();
    for watched in ["modules", "app", "web", ".env"] {
        walk.record(&root.join(watched))?;
    }
    Ok(walk.files)
}

#[derive(Default)]
struct Walk {
    files: Snapshot,
    /// Canonical folders walked, so a symlink back to one is not walked again.
    folders: HashSet<PathBuf>,
}

impl Walk {
    fn record(&mut self, path: &Path) -> Result<(), GenError> {
        let Ok(metadata) = fs::metadata(path) else {
            return Ok(());
        };
        if metadata.is_file() {
            let modified = metadata
                .modified()
                .map_err(|source| io_error(path, source))?;
            self.files.insert(path.to_path_buf(), modified);
            return Ok(());
        }
        let skipped = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| NOT_SOURCES.contains(&name));
        let canonical = fs::canonicalize(path).map_err(|source| io_error(path, source))?;
        if skipped || !self.folders.insert(canonical) {
            return Ok(());
        }
        for entry in fs::read_dir(path).map_err(|source| io_error(path, source))? {
            let entry = entry.map_err(|source| io_error(path, source))?;
            self.record(&entry.path())?;
        }
        Ok(())
    }
}

fn classify(before: &Snapshot, after: &Snapshot) -> Option<Change> {
    if before == after {
        return None;
    }
    if before.len() != after.len() || before.keys().ne(after.keys()) {
        return Some(Change::Code);
    }
    let edited: Vec<&PathBuf> = after
        .iter()
        .filter(|(path, modified)| before.get(*path) != Some(modified))
        .map(|(path, _)| path)
        .collect();
    let has_extension = |extension: &str| {
        edited
            .iter()
            .all(|path| path.extension().is_some_and(|found| found == extension))
    };
    Some(if has_extension("html") {
        Change::Templates
    } else if has_extension("svelte") {
        Change::Islands
    } else {
        Change::Code
    })
}

#[cfg(test)]
mod tests {
    use sofie_model::testing::TemporaryDirectory;

    use super::*;

    fn at(files: &[(&str, u64)]) -> Snapshot {
        files
            .iter()
            .map(|(path, seconds)| {
                (
                    PathBuf::from(path),
                    SystemTime::UNIX_EPOCH + Duration::from_secs(*seconds),
                )
            })
            .collect()
    }

    #[test]
    fn an_edit_is_told_apart_by_what_it_touched() {
        let before = at(&[("a.html", 1), ("b.svelte", 1), ("c.rs", 1)]);

        assert_eq!(classify(&before, &before), None);
        assert_eq!(
            classify(&before, &at(&[("a.html", 2), ("b.svelte", 1), ("c.rs", 1)])),
            Some(Change::Templates)
        );
        assert_eq!(
            classify(&before, &at(&[("a.html", 1), ("b.svelte", 2), ("c.rs", 1)])),
            Some(Change::Islands)
        );
        assert_eq!(
            classify(&before, &at(&[("a.html", 2), ("b.svelte", 2), ("c.rs", 1)])),
            Some(Change::Code)
        );
        assert_eq!(
            classify(
                &before,
                &at(&[("a.html", 1), ("b.svelte", 1), ("d.html", 1)])
            ),
            Some(Change::Code),
            "a new template needs the wiring"
        );
        let mail = at(&[("modules/order/src/mails/receipt.html", 1)]);
        assert_eq!(
            classify(&mail, &at(&[("modules/order/src/mails/receipt.html", 2)])),
            Some(Change::Templates),
            "a development build rereads a mail template as a screen's"
        );
    }

    #[test]
    fn development_refuses_a_database_on_another_host() {
        let urls = |url: &str| BTreeMap::from([("DATABASE_URL".to_owned(), url.to_owned())]);

        let refused = super::refuse_remote_development(&urls("postgres://db.example.com/shop"));

        assert!(
            refused
                .unwrap_err()
                .to_string()
                .contains("--remote-database")
        );
        assert!(super::refuse_remote_development(&urls("postgres://127.0.0.1/shop")).is_ok());
        assert!(super::refuse_remote_development(&BTreeMap::new()).is_ok());
    }

    #[test]
    fn the_env_file_fills_what_the_environment_does_not_set() {
        let directory = TemporaryDirectory::new("sofie-dev-env");
        directory.write(
            ".env",
            "# keys\nSOFIE_DEV_TEST_KEY=\"a=b\" # quoted\n\nPATH=/nowhere\n\
             export SOFIE_DEV_TEST_EXPORTED='x y'\nSOFIE_DEV_TEST_NOTED=v#1 and 2 # a note\n",
        );

        let keys = dotenv(directory.root()).unwrap();

        assert_eq!(
            keys,
            [
                ("SOFIE_DEV_TEST_KEY".to_owned(), "a=b".to_owned()),
                ("SOFIE_DEV_TEST_EXPORTED".to_owned(), "x y".to_owned()),
                ("SOFIE_DEV_TEST_NOTED".to_owned(), "v#1 and 2".to_owned()),
            ],
            "read as `sofie config check` reads it"
        );
        assert_eq!(dotenv(&directory.path("missing")).unwrap(), []);
    }

    #[test]
    fn an_app_command_runs_in_the_app_root_with_its_env_file() {
        let directory = TemporaryDirectory::new("sofie-app-command");
        directory.write(".env", "SOFIE_APP_COMMAND_TEST_KEY=local\n");

        let command = app_command(directory.root(), Path::new("app")).unwrap();

        assert_eq!(command.get_current_dir(), Some(directory.root()));
        let variables: Vec<_> = command.get_envs().collect();
        assert_eq!(
            variables,
            [(
                std::ffi::OsStr::new("SOFIE_APP_COMMAND_TEST_KEY"),
                Some(std::ffi::OsStr::new("local"))
            )]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_back_to_a_watched_folder_is_walked_once() {
        let directory = TemporaryDirectory::new("sofie-dev-loop");
        directory.write("modules/order/src/lib.rs", "");
        std::os::unix::fs::symlink(
            directory.path("modules"),
            directory.path("modules/order/src/back"),
        )
        .unwrap();

        let files = snapshot(directory.root()).unwrap();

        assert_eq!(
            files.keys().collect::<Vec<_>>(),
            [&directory.path("modules/order/src/lib.rs")]
        );
    }

    /// Needs Node and `npm ci` in the web toolchain.
    #[cfg(feature = "slow-tests")]
    #[test]
    fn a_template_edit_is_compiled_into_the_development_client_bundle() {
        use sofie_model::testing::{app, area, define_area, module};

        let directory = TemporaryDirectory::new("sofie-dev-clients");
        let template = directory.write(
            "modules/notes/src/areas/board/screens/notes.html",
            "<p>First words</p>",
        );
        let mut notes = module("notes", directory.path("modules/notes"));
        define_area(&mut notes, "board");
        notes.defined_areas[0].client = sofie_model::AreaClient::Svelte;
        let mut board = area(&notes, "board", Vec::new());
        board.screens = vec![template.clone()];
        notes.areas.push(board);
        let demo = app(directory.root(), vec![notes]);
        let bundle = directory.path("dev/clients/board.js");

        build_development_clients(&demo, &directory.path("dev"), Diagnostics::Terminal).unwrap();
        assert!(fs::read_to_string(&bundle).unwrap().contains("First words"));
        fs::write(&template, "<p>Second words</p>").unwrap();
        build_development_clients(&demo, &directory.path("dev"), Diagnostics::Terminal).unwrap();

        assert!(
            fs::read_to_string(&bundle)
                .unwrap()
                .contains("Second words")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_watch_names_the_app_and_its_pid() {
        let untracked = ChildProcess::untracked();
        let pid = untracked.pid();
        let mut app = DevApp::new(Path::new("/app"), None, AppOutput::Terminal);
        app.process = Some(untracked);

        let error = app.exited().unwrap_err().to_string();

        assert_eq!(
            error,
            format!(
                "watching the app (pid {pid}) failed: child process {pid} is no longer tracked"
            )
        );
    }
}
