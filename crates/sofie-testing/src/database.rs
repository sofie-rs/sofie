use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::JoinHandle;

use sofie_core::unpoisoned;
use tokio::sync::{OnceCell, oneshot};
use tokio_postgres::{Client, NoTls};

const SERVER_URL_VARIABLE: &str = "SOFIE_TEST_DATABASE_URL";
const DEFAULT_SERVER_URL: &str = "postgres://sofie:sofie@127.0.0.1:25432/sofie";
const NAME_PREFIX: &str = "sofie_test_";
/// Held per process for its lifetime; a database whose process no longer holds it was left behind and is dropped.
const LIVENESS_CLASS: i32 = 0x6677_7465;

static NEXT_DATABASE: AtomicU32 = AtomicU32::new(0);
static LIVENESS: OnceCell<()> = OnceCell::const_new();
static PENDING_DROPS: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

pub struct TestDatabase {
    name: String,
    server_url: String,
    url: String,
}

impl TestDatabase {
    pub async fn create() -> Self {
        let server_url =
            std::env::var(SERVER_URL_VARIABLE).unwrap_or_else(|_| DEFAULT_SERVER_URL.to_owned());
        LIVENESS
            .get_or_init(|| hold_liveness(server_url.clone()))
            .await;
        let name = format!(
            "{NAME_PREFIX}{}_{}",
            std::process::id(),
            NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
        );
        run_on_server(
            &server_url,
            &[drop_statement(&name), format!("CREATE DATABASE {name}")],
        )
        .await;
        Self {
            url: with_database(&server_url, &name),
            name,
            server_url,
        }
    }

    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }
}

/// Never blocks: Postgres waits for a half-open connection of the test's runtime, which runs only once the runtime does.
impl Drop for TestDatabase {
    fn drop(&mut self) {
        let statement = drop_statement(&self.name);
        let server_url = self.server_url.clone();
        let dropping = std::thread::spawn(move || {
            on_own_runtime(run_on_server(&server_url, &[statement]));
        });
        unpoisoned::lock(&PENDING_DROPS).push(dropping);
    }
}

/// Called outside a runtime once connections are closed; a drop still running at exit is left to the next test process.
pub fn finish_teardown() {
    let pending = std::mem::take(&mut *unpoisoned::lock(&PENDING_DROPS));
    for dropping in pending {
        if let Err(panic) = dropping.join() {
            std::panic::resume_unwind(panic);
        }
    }
}

fn on_own_runtime(work: impl Future<Output = ()>) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("building a runtime for the test server")
        .block_on(work);
}

async fn hold_liveness(server_url: String) {
    let (locked, held) = oneshot::channel();
    std::thread::spawn(move || {
        on_own_runtime(async move {
            let (client, connection) = connect(&server_url).await;
            let connection = tokio::spawn(connection);
            client
                .execute(
                    "SELECT pg_advisory_lock($1, $2)",
                    &[&LIVENESS_CLASS, &own_process()],
                )
                .await
                .expect("taking the test process's liveness lock");
            let _ = locked.send(());
            drop_abandoned(&client).await;
            let _ = connection.await;
        });
    });
    held.await
        .expect("the liveness connection takes its lock or panics");
}

/// Dropped by the process that takes the dead one's lock, so two processes never drop the same one.
async fn drop_abandoned(client: &Client) {
    let databases = client
        .query(
            "SELECT datname FROM pg_database WHERE datname LIKE $1",
            &[&format!("{NAME_PREFIX}%")],
        )
        .await
        .expect("listing the test databases");
    let mut dead = Vec::new();
    for row in databases {
        let name: String = row.get(0);
        let Some(process) = process_of(&name) else {
            continue;
        };
        if process != own_process() && !dead.contains(&process) && takes_lock(client, process).await
        {
            dead.push(process);
        }
        if dead.contains(&process) {
            client
                .batch_execute(&drop_statement(&name))
                .await
                .unwrap_or_else(|error| panic!("dropping abandoned database {name}: {error}"));
        }
    }
}

async fn takes_lock(client: &Client, process: i32) -> bool {
    client
        .query_one(
            "SELECT pg_try_advisory_lock($1, $2)",
            &[&LIVENESS_CLASS, &process],
        )
        .await
        .expect("trying a test process's liveness lock")
        .get(0)
}

fn drop_statement(name: &str) -> String {
    format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)")
}

fn own_process() -> i32 {
    i32::try_from(std::process::id()).expect("a process id fits an i32")
}

fn process_of(name: &str) -> Option<i32> {
    name.strip_prefix(NAME_PREFIX)?
        .split_once('_')?
        .0
        .parse()
        .ok()
}

async fn connect(
    server_url: &str,
) -> (
    Client,
    tokio_postgres::Connection<tokio_postgres::Socket, tokio_postgres::tls::NoTlsStream>,
) {
    tokio_postgres::connect(server_url, NoTls)
        .await
        .expect("connecting to the test server; is `docker compose up -d --wait` running?")
}

// Databases cannot be created or dropped in the implicit transaction of a multi-statement script.
async fn run_on_server(server_url: &str, statements: &[String]) {
    let (client, connection) = connect(server_url).await;
    let connection = tokio::spawn(connection);
    for sql in statements {
        client
            .batch_execute(sql)
            .await
            .unwrap_or_else(|error| panic!("running `{sql}`: {error}"));
    }
    drop(client);
    connection
        .await
        .expect("joining the connection task")
        .expect("closing the connection");
}

fn with_database(server_url: &str, name: &str) -> String {
    let (base, options) = match server_url.split_once('?') {
        Some((base, options)) => (base, Some(options)),
        None => (server_url, None),
    };
    let host_start = base.find("://").map_or(0, |index| index + 3);
    let path_start = base[host_start..]
        .find('/')
        .map_or(base.len(), |index| host_start + index);
    let mut url = format!("{}/{name}", &base[..path_start]);
    if let Some(options) = options {
        url.push('?');
        url.push_str(options);
    }
    url
}

#[cfg(test)]
mod tests {
    use super::{process_of, with_database};

    #[test]
    fn replaces_only_the_database_of_the_server_url() {
        for (server_url, expected) in [
            (
                "postgres://sofie:sofie@127.0.0.1:25432/sofie",
                "postgres://sofie:sofie@127.0.0.1:25432/t",
            ),
            ("postgres://db.test", "postgres://db.test/t"),
            (
                "postgres://a@db.test/x?sslmode=disable",
                "postgres://a@db.test/t?sslmode=disable",
            ),
        ] {
            assert_eq!(with_database(server_url, "t"), expected, "{server_url}");
        }
    }

    #[test]
    fn a_test_database_is_named_after_its_process() {
        assert_eq!(process_of("sofie_test_4242_7"), Some(4242));
        assert_eq!(process_of("sofie_test_x_7"), None);
        assert_eq!(process_of("sofie"), None);
    }
}
