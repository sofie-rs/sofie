use std::sync::Mutex;

use sofie_core::{Result, TestClock, Timestamp};
use sofie_db::worker::Worker;
use sofie_db::{Db, Migrations};
use sofie_http::App;
use sofie_postgres::Postgres;
use sofie_values::Seed;

use crate::wiring::{AppWiring, Flags, WorkerWiring};
use crate::{TestApp, TestDatabase, TestServer};

/// Fixtures are the same in every run unless this names another seed.
const SEED_VARIABLE: &str = "SOFIE_TEST_SEED";
const DEFAULT_SEED: u64 = 0x5EED;
/// The test clock starts at the real now unless this pins an RFC 3339 instant.
const NOW_VARIABLE: &str = "SOFIE_TEST_NOW";
#[cfg(feature = "flags")]
const FLAGS_ENVIRONMENT: &str = "test";

/// A test whose app needs tables migrates them in `app`, as the binary does only with `SOFIE_MIGRATE=on`.
pub async fn start<Built>(
    app: impl FnOnce(Db) -> Built,
    worker: impl FnOnce(Db) -> Worker,
) -> TestApp
where
    Built: Future<Output = Result<App>>,
{
    wired(&[], app, worker).await
}

#[cfg(feature = "flags")]
pub async fn start_with_flags<Built>(
    app: impl FnOnce(Db, Flags) -> Built,
    worker: impl FnOnce(Db, &Flags) -> Worker,
) -> TestApp
where
    Built: Future<Output = Result<App>>,
{
    wired(&[], app, worker).await
}

pub(crate) async fn wired<A, W, Application, Jobs>(
    migrations: &[Migrations],
    app: Application,
    worker: Jobs,
) -> TestApp
where
    Application: AppWiring<A>,
    Jobs: WorkerWiring<W>,
{
    sofie::fakes_from_tests();
    let database = TestDatabase::create().await;
    let clock = TestClock::starting_at(start_time(std::env::var(NOW_VARIABLE).ok()));
    let db = Db::new(connect(database.url())).with_clock(clock.clock());
    sofie_db::keep_raised(&db);
    db.schema()
        .migrate(migrations)
        .await
        .expect("migrating the test database");
    sofie::install_for_tests(&db).expect("installing the framework's packages");
    // Replaces the framework's fake transport with one whose outbox `sent_mails` reads.
    #[cfg(feature = "mail")]
    let outbox = sofie_mail::testing::fake(&db);
    let flags = start_flags(&db, Application::FLAGS).await;
    let built: Result<App> = app.build(db.clone(), flags.as_ref()).await;
    let server = TestServer::start(built.expect("building the app")).await;
    let postgres = connect(database.url());
    // A worker a test builds from `app.db()` signs links like the app's own.
    let db = db.with_signing_keys(server.signing_keys());
    let worker = sofie::framework_jobs(worker.build(db.clone(), flags.as_ref()));
    sofie_db::worker::publish_heard(&worker);
    TestApp {
        worker,
        #[cfg(feature = "mail")]
        outbox,
        db,
        clock,
        postgres,
        server,
        seed: Mutex::new(Seed::new(seed())),
        #[cfg(feature = "flags")]
        flags,
        _database: database,
    }
}

fn seed() -> u64 {
    std::env::var(SEED_VARIABLE).map_or(DEFAULT_SEED, |seed| {
        seed.parse()
            .unwrap_or_else(|_| panic!("{SEED_VARIABLE} must be a number, got `{seed}`"))
    })
}

fn start_time(pinned: Option<String>) -> Timestamp {
    pinned.map_or_else(Timestamp::now, |now| {
        now.parse().unwrap_or_else(|error| {
            panic!(
                "{NOW_VARIABLE} must be an RFC 3339 instant such as 2026-01-05T09:00:00Z, \
                 got `{now}`: {error}"
            )
        })
    })
}

async fn start_flags(db: &Db, wanted: bool) -> Option<Flags> {
    #[cfg(feature = "flags")]
    if wanted {
        return Some(
            Flags::start(db, FLAGS_ENVIRONMENT)
                .await
                .expect("starting the feature flags"),
        );
    }
    #[cfg(not(feature = "flags"))]
    let _ = (db, wanted);
    None
}

fn connect(url: &str) -> Postgres {
    Postgres::connect(url).expect("connecting to the test database")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pinned_now_starts_the_test_clock() {
        let pinned = start_time(Some("2026-01-05T09:00:00+03:00".to_owned()));

        assert_eq!(pinned, "2026-01-05T06:00:00Z".parse::<Timestamp>().unwrap());
    }

    #[test]
    #[should_panic(expected = "SOFIE_TEST_NOW must be an RFC 3339 instant")]
    fn a_now_that_is_no_instant_fails_naming_the_variable() {
        start_time(Some("next monday".to_owned()));
    }
}
