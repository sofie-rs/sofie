use std::sync::OnceLock;

use sofie_core::{Clock, Error, Result};
use sofie_db::Db;
use sofie_db::worker::Worker;
use sofie_flags::Flags;
use sofie_http::{App, Module};

const SECONDS_PER_DAY: i64 = 86_400;
const ENVIRONMENT: &str = "SOFIE_ENVIRONMENT";

pub fn run_with_flags<Built>(
    modules: &[Module],
    app: impl FnOnce(Db, Flags) -> Built,
    worker: impl FnOnce(Db, &Flags) -> Worker,
) -> Result<()>
where
    Built: Future<Output = Result<App>>,
{
    flagged(modules, app, worker, crate::NO_TASKS)
}

#[cfg(feature = "tasks")]
pub fn run_with_flags_and_tasks<Built>(
    modules: &[Module],
    app: impl FnOnce(Db, Flags) -> Built,
    worker: impl FnOnce(Db, &Flags) -> Worker,
    tasks: fn() -> crate::Tasks,
) -> Result<()>
where
    Built: Future<Output = Result<App>>,
{
    flagged(modules, app, worker, Some(tasks))
}

/// As `sofie::run_worker`, for an app whose worker takes the flags.
pub fn run_worker_with_flags(
    modules: &[Module],
    worker: impl FnOnce(Db, &Flags) -> Worker,
) -> Result<()> {
    let started = OnceLock::new();
    let worker = worker_builder(&started, sofie_core::environment_variable, worker);
    crate::exit_on_failure(crate::serve_worker(modules, worker))
}

fn flagged<Built>(
    modules: &[Module],
    app: impl FnOnce(Db, Flags) -> Built,
    worker: impl FnOnce(Db, &Flags) -> Worker,
    tasks: crate::Registered,
) -> Result<()>
where
    Built: Future<Output = Result<App>>,
{
    let started = OnceLock::new();
    let (app, worker) = builders(&started, sofie_core::environment_variable, app, worker);
    crate::launch(modules, app, worker, tasks)
}

type Lookup = fn(&str) -> Result<Option<String>>;

/// A process serving both roles shares one `Flags`, started by whichever is built first; a
/// command such as `schedule run` builds only the worker.
fn builders<'a, Built>(
    started: &'a OnceLock<Flags>,
    lookup: Lookup,
    app: impl FnOnce(Db, Flags) -> Built + 'a,
    worker: impl FnOnce(Db, &Flags) -> Worker + 'a,
) -> (crate::AppBuilder<'a>, crate::WorkerBuilder<'a>)
where
    Built: Future<Output = Result<App>> + 'a,
{
    let app = crate::app_builder(move |db| async move {
        let flags = shared(started, &db, lookup).await?;
        app(db, flags).await
    });
    (app, worker_builder(started, lookup, worker))
}

fn worker_builder<'a>(
    started: &'a OnceLock<Flags>,
    lookup: Lookup,
    worker: impl FnOnce(Db, &Flags) -> Worker + 'a,
) -> crate::WorkerBuilder<'a> {
    Box::new(move |db| {
        Box::pin(async move {
            let flags = shared(started, &db, lookup).await?;
            Ok(worker(db, &flags))
        })
    })
}

async fn shared(started: &OnceLock<Flags>, db: &Db, lookup: Lookup) -> Result<Flags> {
    if let Some(flags) = started.get() {
        return Ok(flags.clone());
    }
    let flags = start(db, lookup).await?;
    Ok(started.get_or_init(|| flags).clone())
}

async fn start(db: &Db, lookup: Lookup) -> Result<Flags> {
    Flags::start(db, &environment(lookup(ENVIRONMENT)?)?).await
}

/// Required: a guessed environment could switch on in production a flag meant for staging.
fn environment(value: Option<String>) -> Result<String> {
    value.filter(|name| !name.trim().is_empty()).ok_or_else(|| {
        Error::internal_message(format!(
            "{ENVIRONMENT} is not set; name the environment flags are evaluated in, such as `production`"
        ))
    })
}

pub(crate) async fn print_settled(db: &Db) -> Result<()> {
    let flags = start(db, sofie_core::environment_variable).await?;
    let now = Clock::system().now().timestamp();
    for flag in flags.settled() {
        let days = now.duration_since(flag.since).as_secs().max(0) / SECONDS_PER_DAY;
        let state = if flag.on { "on" } else { "off" };
        println!("{} {state} {days}", flag.name);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMED_ENVIRONMENT: Lookup = |_| Ok(Some("test".to_owned()));

    async fn migrated() -> Db {
        let db = Db::new(sofie_sqlite::Sqlite::memory().unwrap());
        db.schema().migrate(&[]).await.unwrap();
        db
    }

    #[tokio::test]
    async fn a_worker_built_without_the_app_starts_the_flags() {
        let started = OnceLock::new();
        let (_, worker) = builders(
            &started,
            NAMED_ENVIRONMENT,
            |db, _| async { Ok(App::new(db)) },
            |db, _| Worker::new(db),
        );

        worker(migrated().await).await.unwrap();

        assert!(started.get().is_some());
    }

    #[tokio::test]
    async fn a_worker_only_binary_hands_its_worker_the_flags() {
        let started = OnceLock::new();
        let handed = std::cell::Cell::new(false);
        let worker = worker_builder(&started, NAMED_ENVIRONMENT, |db, _flags| {
            handed.set(true);
            Worker::new(db)
        });

        worker(migrated().await).await.unwrap();

        assert!(handed.get());
        assert!(started.get().is_some());
    }

    #[test]
    fn the_environment_flags_are_evaluated_in_must_be_named() {
        assert_eq!(environment(Some("staging".to_owned())).unwrap(), "staging");
        for unnamed in [None, Some(" ".to_owned())] {
            let error = environment(unnamed).unwrap_err();
            assert!(error.to_string().starts_with(ENVIRONMENT), "{error}");
        }
    }
}
