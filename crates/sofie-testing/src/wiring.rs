use sofie_core::Result;
use sofie_db::Db;
use sofie_db::worker::Worker;
use sofie_http::{App, Module};

use crate::{TestApp, finish_teardown};

pub trait AppWiring<Shape> {
    const FLAGS: bool;

    fn build(self, db: Db, flags: Option<&Flags>) -> impl Future<Output = Result<App>>;
}

pub trait WorkerWiring<Shape> {
    fn build(self, db: Db, flags: Option<&Flags>) -> Worker;
}

#[cfg(feature = "flags")]
pub use sofie_flags::Flags;

#[cfg(not(feature = "flags"))]
pub enum Flags {}

impl<F, Built> AppWiring<fn(Db)> for F
where
    F: FnOnce(Db) -> Built,
    Built: Future<Output = Result<App>>,
{
    const FLAGS: bool = false;

    fn build(self, db: Db, _flags: Option<&Flags>) -> impl Future<Output = Result<App>> {
        self(db)
    }
}

impl<F> WorkerWiring<fn(Db)> for F
where
    F: FnOnce(Db) -> Worker,
{
    fn build(self, db: Db, _flags: Option<&Flags>) -> Worker {
        self(db)
    }
}

#[cfg(feature = "flags")]
impl<F, Built> AppWiring<fn(Db, Flags)> for F
where
    F: FnOnce(Db, Flags) -> Built,
    Built: Future<Output = Result<App>>,
{
    const FLAGS: bool = true;

    fn build(self, db: Db, flags: Option<&Flags>) -> impl Future<Output = Result<App>> {
        let flags = flags
            .expect("the flags start before an app that takes them")
            .clone();
        self(db, flags)
    }
}

#[cfg(feature = "flags")]
impl<F> WorkerWiring<fn(Db, &Flags)> for F
where
    F: FnOnce(Db, &Flags) -> Worker,
{
    fn build(self, db: Db, flags: Option<&Flags>) -> Worker {
        self(
            db,
            flags.expect("a worker taking the flags runs beside an app taking them"),
        )
    }
}

/// The runtime is shut down before the database is dropped, so no app connection holds the drop up.
pub fn run_test_app<AppShape, WorkerShape, Test>(
    modules: &[Module],
    app: impl AppWiring<AppShape>,
    worker: impl WorkerWiring<WorkerShape>,
    test: impl FnOnce(TestApp) -> Test,
) where
    Test: Future<Output = ()>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("building the test's runtime");
    runtime.block_on(async {
        let app = crate::start::wired(&sofie_http::migrations(modules), app, worker).await;
        test(app).await;
    });
    drop(runtime);
    finish_teardown();
}
