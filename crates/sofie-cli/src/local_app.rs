use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::hash::{BuildHasher, RandomState};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use sofie_model::SignInPaths;

use crate::child_process::stop_on_interrupt;
use crate::dev_command::{AppOutput, DevApp, dotenv};
use crate::gen_command::{GenError, io_error};
use crate::http_client::{self, CookieJars, HttpError, Request, Response};
use crate::log_buffer::LogBuffer;

const STARTUP: Duration = Duration::from_mins(1);
const STARTUP_POLL: Duration = Duration::from_millis(100);
const TAIL_ON_EXIT: usize = 40;
/// The response can arrive before the request's span closes and is logged.
const TREE_WAIT: Duration = Duration::from_secs(2);
const TREE_POLL: Duration = Duration::from_millis(20);
const DATABASE_URL: &str = "DATABASE_URL";
/// `sofie bench` signs in as the members `sofie seed` made records as, so the
/// load reads them. `bench/run.sh` reads it from this line.
pub(crate) const MEMBER_PASSWORD: &str = "bench-password-123";

pub(crate) fn bench_member(area: &str) -> String {
    format!("bench-{area}@example.com")
}

pub(crate) enum StartError {
    Exited(ExitStatus),
    Unwatchable(GenError),
    Silent {
        path: &'static str,
        last: Option<HttpError>,
    },
}

impl fmt::Display for StartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exited(status) => write!(formatter, "the app exited with {status}"),
            Self::Unwatchable(error) => write!(formatter, "{error}"),
            Self::Silent { path, last } => {
                write!(
                    formatter,
                    "the app did not answer `{path}` within {STARTUP:?}"
                )?;
                match last {
                    Some(error) => write!(formatter, " ({error})"),
                    None => Ok(()),
                }
            }
        }
    }
}

/// The development app on a free local port with its output kept, as `sofie
/// fuzz`, `sofie seed` and `sofie mcp` run it.
pub(crate) struct LocalApp {
    pub(crate) app: DevApp,
    pub(crate) address: SocketAddr,
    pub(crate) log: Arc<LogBuffer>,
}

impl LocalApp {
    pub(crate) fn new(root: &Path, log: Arc<LogBuffer>) -> Result<Self, GenError> {
        let address = free_address()?;
        let output = AppOutput::Captured {
            log: Arc::clone(&log),
            address,
        };
        Ok(Self {
            app: DevApp::new(root, None, output),
            address,
            log,
        })
    }

    /// For generated traffic: the shield off, so it is never blocked.
    pub(crate) fn start(root: &Path, command: &str) -> Result<Self, GenError> {
        refuse_remote_database(root, command)?;
        stop_on_interrupt().map_err(|source| io_error(root, source))?;
        let mut local = Self::new(root, Arc::default())?;
        local.app.set_variable("SOFIE_SHIELD", "off");
        local.restart()?;
        Ok(local)
    }

    pub(crate) fn restart(&mut self) -> Result<(), GenError> {
        self.app.rebuild()?;
        self.wait_live()
    }

    pub(crate) fn wait_live(&mut self) -> Result<(), GenError> {
        let app = &mut self.app;
        wait_answering(self.address, "/live", || app.exited()).map_err(|error| match error {
            StartError::Unwatchable(error) => error,
            StartError::Exited(_) => GenError::App(format!(
                "{error}; its last lines:\n{}",
                self.log.tail(TAIL_ON_EXIT).join("\n")
            )),
            StartError::Silent { .. } => GenError::App(error.to_string()),
        })
    }

    pub(crate) fn send(&self, request: &Request) -> Result<Response, HttpError> {
        http_client::send(self.address, request)
    }

    pub(crate) fn send_json(
        &self,
        method: &str,
        path: &str,
        cookie: Option<String>,
        body: &str,
    ) -> Result<Response, HttpError> {
        let mut headers = vec![
            ("accept", "application/json".to_owned()),
            ("content-type", "application/json".to_owned()),
            ("origin", self.origin()),
        ];
        headers.extend(cookie.map(|cookie| ("cookie", cookie)));
        self.send(&Request {
            method,
            path,
            headers,
            body: body.as_bytes(),
        })
    }

    pub(crate) fn origin(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Signing up again changes nothing and signing in still works, so one
    /// member serves every run.
    pub(crate) fn sign_in(
        &self,
        jars: &mut CookieJars,
        jar: &str,
        paths: &SignInPaths,
        email: &str,
    ) -> Result<(), GenError> {
        let credentials = serde_json::json!({ "email": email, "password": MEMBER_PASSWORD });
        let body = credentials.to_string();
        let post = |path: &str| {
            self.send_json("POST", path, None, &body)
                .map_err(|error| GenError::App(format!("signing in at {path}: {error}")))
        };
        if let Some(sign_up) = &paths.sign_up {
            post(sign_up)?;
        }
        let response = post(&paths.sign_in)?;
        jars.store(jar, response.headers("set-cookie"));
        if jars.cookie_header(jar).is_none() {
            return Err(GenError::App(format!(
                "signing in as {email} at {} answered {} without a session",
                paths.sign_in, response.status
            )));
        }
        Ok(())
    }
}

/// For a command whose app writes, as `db fresh`'s drops do.
pub(crate) fn refuse_remote_database(root: &Path, command: &str) -> Result<(), GenError> {
    refuse_remote(&database_urls(root)?, command)
}

/// `DATABASE_URL` and each module's own `<MODULE>_DATABASE_URL`, the
/// environment's over `.env`'s.
pub(crate) fn database_urls(root: &Path) -> Result<BTreeMap<String, String>, GenError> {
    let mut urls: BTreeMap<String, String> = dotenv(root)?
        .into_iter()
        .filter(|(key, _)| is_database_key(key))
        .collect();
    urls.extend(std::env::vars().filter(|(key, _)| is_database_key(key)));
    Ok(urls)
}

fn is_database_key(key: &str) -> bool {
    key == DATABASE_URL || key.ends_with("_DATABASE_URL")
}

/// A SQLite database is a file or memory of this machine.
pub(crate) fn remote_database_key(urls: &BTreeMap<String, String>) -> Option<&str> {
    urls.iter()
        .find(|(_, url)| !url.starts_with("sqlite:") && !sofie_core::is_local_database(url))
        .map(|(key, _)| key.as_str())
}

fn refuse_remote(urls: &BTreeMap<String, String>, command: &str) -> Result<(), GenError> {
    if let Some(key) = remote_database_key(urls) {
        return Err(GenError::Usage(format!(
            "`sofie {command}` writes to the app's database, so it runs only on a database on this \
             machine (localhost, 127.0.0.1, ::1 or a socket), and {key} names another host"
        )));
    }
    if urls.is_empty() {
        return Err(GenError::Usage(format!(
            "`sofie {command}` needs DATABASE_URL from the environment or `.env`"
        )));
    }
    Ok(())
}

pub(crate) fn free_address() -> Result<SocketAddr, GenError> {
    let localhost = SocketAddr::from(([127, 0, 0, 1], 0));
    TcpListener::bind(localhost)
        .and_then(|listener| listener.local_addr())
        .map_err(|error| GenError::App(format!("no free local port for the app: {error}")))
}

/// Until `path` answers 200; `exited` tells a crashed app from a slow start.
pub(crate) fn wait_answering(
    address: SocketAddr,
    path: &'static str,
    mut exited: impl FnMut() -> Result<Option<ExitStatus>, GenError>,
) -> Result<(), StartError> {
    let probe = Request {
        method: "GET",
        path,
        headers: Vec::new(),
        body: b"",
    };
    let started = Instant::now();
    let mut last = None;
    while started.elapsed() < STARTUP {
        if let Some(status) = exited().map_err(StartError::Unwatchable)? {
            return Err(StartError::Exited(status));
        }
        match http_client::send(address, &probe) {
            Ok(response) if response.status == 200 => return Ok(()),
            Ok(_) => {}
            Err(error) => last = Some(error),
        }
        std::thread::sleep(STARTUP_POLL);
    }
    Err(StartError::Silent { path, last })
}

fn random() -> u64 {
    RandomState::new().hash_one(SystemTime::now()).max(1)
}

/// A new directory only this user may enter, under a name nobody could
/// prepare: in a shared temporary directory another user could otherwise
/// create the path first or plant a symlink at it.
pub(crate) fn private_directory(prefix: &str) -> Result<PathBuf, GenError> {
    let path = std::env::temp_dir().join(format!("{prefix}-{:016x}", random()));
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(&path)
        .map_err(|source| io_error(&path, source))?;
    Ok(path)
}

pub(crate) fn wait_for_trees(log: &LogBuffer, trace_id: &str) -> Vec<String> {
    let started = Instant::now();
    while log.trees(trace_id).trees.is_empty() && started.elapsed() < TREE_WAIT {
        std::thread::sleep(TREE_POLL);
    }
    // A tree's later lines may still be in the pipe behind its first.
    std::thread::sleep(TREE_POLL);
    log.trees(trace_id)
        .trees
        .into_iter()
        .map(|tree| tree.lines.join("\n"))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::refuse_remote;

    fn urls(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, url)| ((*key).to_owned(), (*url).to_owned()))
            .collect()
    }

    #[test]
    fn generated_traffic_writes_only_to_a_database_on_this_machine() {
        let local = urls(&[("DATABASE_URL", "postgres://localhost/shop")]);
        assert!(refuse_remote(&local, "bench").is_ok());
        assert!(refuse_remote(&urls(&[("DATABASE_URL", "sqlite://shop.db")]), "seed").is_ok());

        let remote = urls(&[("DATABASE_URL", "postgres://db.example.com/shop")]);
        let remote = refuse_remote(&remote, "bench").unwrap_err();
        let missing = refuse_remote(&urls(&[]), "build --pgo").unwrap_err();

        assert!(
            remote.to_string().starts_with("`sofie bench` writes"),
            "{remote}"
        );
        assert!(
            missing
                .to_string()
                .contains("`sofie build --pgo` needs DATABASE_URL")
        );
    }

    #[test]
    fn a_private_directory_is_new_and_only_its_owner_may_enter_it() {
        let first = super::private_directory("sofie-private").unwrap();
        let second = super::private_directory("sofie-private").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&first).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        assert_ne!(first, second);
        std::fs::remove_dir(&first).unwrap();
        std::fs::remove_dir(&second).unwrap();
    }

    #[test]
    fn a_modules_own_database_on_another_host_is_refused_too() {
        let urls = urls(&[
            ("DATABASE_URL", "postgres://localhost/shop"),
            ("ORDER_DATABASE_URL", "postgres://orders.example.com/orders"),
        ]);

        let refused = refuse_remote(&urls, "seed").unwrap_err();

        assert!(
            refused
                .to_string()
                .contains("ORDER_DATABASE_URL names another host"),
            "{refused}"
        );
    }
}
