extern crate self as sofie;

pub use serde;
pub use serde_json;
pub use sofie_auth as auth;
#[cfg(feature = "billing")]
pub use sofie_billing as billing;
#[cfg(feature = "cache")]
pub use sofie_cache as cache;
pub use sofie_core as core;
/// A module rather than a renamed crate, so rustc suggests `sofie::db::Aggregate` for a trait
/// a derive implements, not the private `sofie::sofie_db` path.
pub mod db {
    pub use sofie_db::*;
}
pub use sofie_db::worker;
#[cfg(feature = "engine")]
pub use sofie_engine as engine;
#[cfg(feature = "feature-flags")]
pub use sofie_flags as flags;
pub use sofie_http as http;
pub use sofie_macros::*;
#[cfg(feature = "notify")]
pub use sofie_notify as notify;
#[cfg(feature = "postgres")]
pub use sofie_postgres as postgres;
#[cfg(feature = "sqlite")]
pub use sofie_sqlite as sqlite;
#[cfg(feature = "storage")]
pub use sofie_storage as storage;
pub use sofie_telemetry as telemetry;
pub use sofie_values as values;

#[cfg(feature = "mimalloc")]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "client")]
pub mod client;
mod command;
#[cfg(feature = "config")]
pub mod config;
mod database;
#[cfg(feature = "development")]
mod database_status;
mod fakes;
#[cfg(feature = "feature-flags")]
mod feature_flags;
#[cfg(feature = "development")]
mod fresh;
mod heard;
mod jobs;
mod local_time;
#[cfg(feature = "mail")]
pub mod mail;
#[cfg(feature = "remote-contracts")]
pub mod remote;
#[cfg(feature = "product-reports")]
mod reports;
#[cfg(feature = "feature-flags")]
mod runtime_settings;
mod schema_check;
#[cfg(feature = "resources")]
mod screen_source;
mod search_settings;
#[cfg(feature = "serving-settings")]
mod serving_settings;
mod settings;
mod signal;
#[cfg(feature = "storage")]
mod storage_settings;
#[cfg(feature = "tasks")]
mod tasks;
mod together;

#[doc(hidden)]
pub use fakes::fakes_from_tests;

use std::net::SocketAddr;
use std::pin::Pin;

use sofie_core::{Error, Result};
use sofie_db::worker::Worker;
use sofie_db::{Db, Migrations};
use sofie_http::App;
use tokio::net::TcpListener;

use crate::command::{Command, Role, WorkerCommand};
use crate::settings::Settings;
#[cfg(feature = "config")]
pub use config::Config;
#[cfg(all(feature = "feature-flags", feature = "tasks"))]
pub use feature_flags::run_with_flags_and_tasks;
#[cfg(feature = "feature-flags")]
pub use feature_flags::{run_with_flags, run_worker_with_flags};
#[cfg(feature = "feature-flags")]
pub use runtime_settings::{RuntimeSettings, declare_settings};
use signal::stop_signal;
pub use sofie_http::{Module, Setting, SettingValue, Tier};
#[cfg(feature = "tasks")]
pub use tasks::{TaskContext, Tasks};

#[cfg(feature = "tasks")]
type Registered = Option<fn() -> Tasks>;
#[cfg(not(feature = "tasks"))]
type Registered = ();
#[cfg(feature = "tasks")]
const NO_TASKS: Registered = None;
#[cfg(not(feature = "tasks"))]
const NO_TASKS: Registered = ();

/// `serve [--role web|worker|all]` (the default), `schema check`, `db fresh` or `db status`.
pub fn run<Built>(
    modules: &[sofie_http::Module],
    app: impl FnOnce(Db) -> Built,
    worker: impl FnOnce(Db) -> Worker,
) -> Result<()>
where
    Built: Future<Output = Result<App>>,
{
    exit_on_failure(launch(
        modules,
        app_builder(app),
        worker_builder(worker),
        NO_TASKS,
    ))
}

/// As `run`, plus `task MODULE.NAME [--name value]...`.
#[cfg(feature = "tasks")]
pub fn run_with_tasks<Built>(
    modules: &[sofie_http::Module],
    app: impl FnOnce(Db) -> Built,
    worker: impl FnOnce(Db) -> Worker,
    tasks: fn() -> Tasks,
) -> Result<()>
where
    Built: Future<Output = Result<App>>,
{
    exit_on_failure(launch(
        modules,
        app_builder(app),
        worker_builder(worker),
        Some(tasks),
    ))
}

/// Boxed at the public entry points, so the startup and serving code behind them is
/// compiled once in this crate instead of again in every app binary on each rebuild.
type AppBuilder<'a> = Box<dyn FnOnce(Db) -> Pin<Box<dyn Future<Output = Result<App>> + 'a>> + 'a>;
type WorkerBuilder<'a> =
    Box<dyn FnOnce(Db) -> Pin<Box<dyn Future<Output = Result<Worker>> + 'a>> + 'a>;

fn app_builder<'a, Built>(app: impl FnOnce(Db) -> Built + 'a) -> AppBuilder<'a>
where
    Built: Future<Output = Result<App>> + 'a,
{
    Box::new(move |db| Box::pin(app(db)))
}

fn worker_builder<'a>(worker: impl FnOnce(Db) -> Worker + 'a) -> WorkerBuilder<'a> {
    Box::new(move |db| Box::pin(std::future::ready(Ok(worker(db)))))
}

fn launch(
    modules: &[sofie_http::Module],
    app: AppBuilder<'_>,
    worker: WorkerBuilder<'_>,
    #[cfg_attr(
        not(feature = "tasks"),
        expect(unused_variables, reason = "only a task reads it")
    )]
    tasks: Registered,
) -> Result<()> {
    settings::install_display_zone()?;
    let command = Command::from_arguments(
        std::env::args_os().skip(1),
        sofie_core::DisplayZone::current(),
    )?;
    let settings = Settings::from_environment()?;
    let migrations = &sofie_http::migrations(modules);
    on_runtime(&settings, async |db| match command {
        #[cfg(feature = "development")]
        Command::FreshDatabase => {
            fresh::fresh_database(&settings.database_url, &db, migrations).await
        }
        #[cfg(feature = "development")]
        Command::DatabaseStatus => database_status::print(&db, migrations).await,
        #[cfg(feature = "development")]
        Command::RunSchedule(name) => {
            let packages = Packages::start(&db, migrations).await?;
            let queued = async {
                let built = packages.worker(&settings, db, worker).await?;
                sofie_db::worker::queue_schedule(&built, &name).await
            }
            .await;
            packages.stop().await;
            queued?;
            println!("queued schedule `{name}`; the running worker runs it next");
            Ok(())
        }
        #[cfg(feature = "feature-flags")]
        Command::SettledFlags => feature_flags::print_settled(&db).await,
        Command::SchemaCheck => schema_check::print(&db, migrations).await,
        Command::Serve(role) => start(role, &settings, db, migrations, app, worker).await,
        #[cfg(feature = "development")]
        Command::ServeAt(instant) => {
            let db = db.with_clock(sofie_core::Clock::starting_at(instant));
            start(Role::All, &settings, db, migrations, app, worker).await
        }
        #[cfg(feature = "resources")]
        Command::ScreenSource(key) => screen_source::print(&key, &app(db).await?),
        #[cfg(feature = "tasks")]
        Command::Task { name, arguments } => {
            let packages = Packages::start(&db, migrations).await?;
            let outcome = tasks::run_by_hand(tasks, db, &name, arguments, app, worker).await;
            packages.stop().await;
            outcome
        }
        #[cfg(feature = "product-reports")]
        Command::Report(retention) => reports::print(&db, &retention).await,
        Command::Jobs(jobs) => jobs.run(db).await,
    })
}

/// A binary that only runs jobs: `serve` (the default) or `schema check`.
/// It never builds the app, so the HTTP server and screens are left out of it.
pub fn run_worker(modules: &[sofie_http::Module], worker: impl FnOnce(Db) -> Worker) -> Result<()> {
    exit_on_failure(serve_worker(modules, worker_builder(worker)))
}

/// `main` returning the error would print it as `Debug`; a person reads its message instead.
fn exit_on_failure(outcome: Result<()>) -> Result<()> {
    let Err(error) = outcome else {
        return Ok(());
    };
    eprintln!("Error: {error}");
    std::process::exit(1)
}

fn serve_worker(modules: &[sofie_http::Module], worker: WorkerBuilder<'_>) -> Result<()> {
    settings::install_display_zone()?;
    let command = WorkerCommand::from_arguments(std::env::args_os().skip(1))?;
    let settings = Settings::from_environment()?;
    let migrations = &sofie_http::migrations(modules);
    on_runtime(&settings, async |db| match command {
        WorkerCommand::Serve => {
            migrate_when_told(&settings, &db, migrations).await?;
            let packages = Packages::start(&db, migrations).await?;
            let outcome = async {
                let worker = packages.worker(&settings, db, worker).await?;
                worker.run(stop_signal()?).await
            }
            .await;
            packages.stop().await;
            outcome
        }
        WorkerCommand::SchemaCheck => schema_check::print(&db, migrations).await,
    })
}

fn on_runtime(settings: &Settings, work: impl AsyncFnOnce(Db) -> Result<()>) -> Result<()> {
    let mut runtime = tokio::runtime::Builder::new_multi_thread();
    if let Some(threads) = settings.runtime_threads {
        runtime.worker_threads(threads.get());
    }
    runtime
        .enable_all()
        .build()
        .map_err(|error| Error::internal("starting the async runtime", error))?
        .block_on(async {
            sofie_telemetry::init()
                .map_err(|error| Error::internal("starting telemetry", error))?;
            let outcome = async {
                settings::warn_on_old_signing_keys()?;
                let db = database::connect(&settings.database_url, settings.pool)?;
                work(db).await
            }
            .await;
            sofie_telemetry::shutdown().await;
            outcome
        })
}

/// Held for the process's life, as the database keeps only weak references to them.
struct Packages {
    #[cfg(feature = "billing")]
    billing: sofie_billing::Setup,
    #[cfg(feature = "cache")]
    _cache: sofie_cache::Cache,
    #[cfg(feature = "mail")]
    mail: sofie_mail::Setup,
    #[cfg(feature = "notify")]
    notify: sofie_notify::Setup,
    #[cfg(feature = "product-reports")]
    product_events: sofie_db::ProductEvents,
    #[cfg(feature = "product-reports")]
    product_events_kept: std::time::Duration,
    #[cfg(feature = "search-meilisearch")]
    search: Option<search_settings::Search>,
}

impl Packages {
    #[cfg_attr(not(feature = "cache"), expect(clippy::unused_async_trait_impl))]
    #[cfg_attr(
        not(feature = "search-meilisearch"),
        expect(
            unused_variables,
            reason = "only a search engine indexes the migrations' aggregates"
        )
    )]
    async fn start(db: &Db, migrations: &[Migrations]) -> Result<Self> {
        #[cfg(not(feature = "cache"))]
        let _ = db;
        #[cfg(not(feature = "search-meilisearch"))]
        search_settings::refuse_unbuilt(&sofie_core::environment_variable)?;
        #[cfg(feature = "search-meilisearch")]
        let search = search_settings::configured(&sofie_core::environment_variable, migrations)?;
        #[cfg(feature = "search-meilisearch")]
        if let Some(search) = &search {
            search.install(db);
        }
        #[cfg(feature = "billing")]
        let billing = sofie_billing::Setup::from_environment()?;
        #[cfg(feature = "billing")]
        sofie_billing::configure(db, billing.clone());
        Ok(Self {
            #[cfg(feature = "billing")]
            billing,
            #[cfg(feature = "cache")]
            _cache: sofie_cache::Cache::from_environment(db).await?,
            #[cfg(feature = "mail")]
            mail: mail::set_up(db)?,
            #[cfg(feature = "notify")]
            notify: notify_set_up(db)?,
            #[cfg(feature = "product-reports")]
            product_events: sofie_db::ProductEvents::keep(db)?,
            #[cfg(feature = "product-reports")]
            product_events_kept: reports::kept()?,
            #[cfg(feature = "search-meilisearch")]
            search,
        })
    }

    /// Writes what the process has queued before it exits.
    #[cfg_attr(not(feature = "product-reports"), expect(clippy::unused_async))]
    async fn stop(&self) {
        #[cfg(feature = "product-reports")]
        self.product_events.flush().await;
    }

    async fn worker(
        &self,
        settings: &Settings,
        db: Db,
        worker: WorkerBuilder<'_>,
    ) -> Result<Worker> {
        let separate = settings
            .worker_pool()
            .map(|pool| database::connect_worker(&settings.database_url, pool))
            .transpose()?
            .flatten();
        let worker_db = match separate {
            Some(separate) => separate.with_clock(db.clock()).sharing_arrivals(&db),
            None => db,
        };
        #[cfg(feature = "signed-links")]
        let worker_db = worker_db.with_signing_keys(std::sync::Arc::new(
            sofie_http::SigningKeys::from_environment()?,
        ));
        #[cfg(feature = "billing")]
        sofie_billing::configure(&worker_db, self.billing.clone());
        // Jobs take it as `Dep<Storage>`, such as one making an upload's image variants.
        #[cfg(feature = "storage")]
        sofie_db::provide(
            &worker_db,
            std::sync::Arc::new(storage_settings::configured(
                &sofie_core::environment_variable,
            )?),
        );
        #[cfg(feature = "notify")]
        sofie_notify::install(&worker_db, &self.notify);
        #[cfg(feature = "mail")]
        sofie_mail::install(&worker_db, &self.mail);
        #[cfg(feature = "search-meilisearch")]
        if let Some(search) = &self.search {
            search.install(&worker_db);
        }
        let worker = framework_jobs(worker(worker_db).await?);
        #[cfg(feature = "search-meilisearch")]
        let worker = if self.search.is_some() {
            sofie_db::index_searches(worker)
        } else {
            worker
        };
        #[cfg(feature = "product-reports")]
        let worker = reports::forgetting(worker, self.product_events_kept);
        sofie_db::worker::publish_heard(&worker);
        Ok(match settings.worker_batch {
            Some(batch) => worker.batch(batch),
            None => worker,
        })
    }
}

/// The packages' jobs, added to the app's worker and to a test's alike, so the two cannot drift.
#[doc(hidden)]
#[must_use]
pub fn framework_jobs(worker: Worker) -> Worker {
    #[cfg(feature = "mail")]
    let worker = sofie_mail::worker(worker);
    #[cfg(feature = "notify")]
    let worker = sofie_notify::worker(worker);
    worker
}

/// What `framework_jobs` needs on a test's database besides the fake mail transport,
/// which sofie-testing installs: notify without web push keys, as no test reaches a push service.
#[doc(hidden)]
pub fn install_for_tests(db: &Db) -> Result<()> {
    #[cfg(feature = "notify")]
    sofie_notify::install(db, &sofie_notify::Setup::new(None)?);
    #[cfg(not(feature = "notify"))]
    let _ = db;
    Ok(())
}

/// Read once from `SOFIE_NOTIFY_*`, so every way the app boots can notify.
#[cfg(feature = "notify")]
fn notify_set_up(db: &Db) -> Result<sofie_notify::Setup> {
    let setup = sofie_notify::Setup::from_environment()?;
    sofie_notify::install(db, &setup);
    Ok(setup)
}

/// A database nothing migrated would fail every query and worker poll, so it is refused once,
/// read from the catalog, which needs no right to write.
async fn migrate_when_told(settings: &Settings, db: &Db, migrations: &[Migrations]) -> Result<()> {
    if settings.migrate {
        return db.schema().migrate(migrations).await;
    }
    if db.schema().framework_migrated().await? == Some(false) {
        return Err(Error::internal_message(
            "the database has none of the framework's tables: start once with \
             `SOFIE_MIGRATE=on`, or migrate it the way the deployment does",
        ));
    }
    Ok(())
}

async fn start(
    role: Role,
    settings: &Settings,
    db: Db,
    migrations: &[Migrations],
    app: AppBuilder<'_>,
    worker: WorkerBuilder<'_>,
) -> Result<()> {
    migrate_when_told(settings, &db, migrations).await?;
    let packages = Packages::start(&db, migrations).await?;
    let outcome = run_role(role, settings, db, &packages, app, worker).await;
    packages.stop().await;
    outcome
}

/// Whatever ends it, even failing at boot, the caller still stops the packages.
async fn run_role(
    role: Role,
    settings: &Settings,
    db: Db,
    packages: &Packages,
    app: AppBuilder<'_>,
    worker: WorkerBuilder<'_>,
) -> Result<()> {
    // Built in every role, so a worker fails at boot on the config and
    // screens a web process would fail on.
    let app = app(db.clone()).await?;
    #[cfg(feature = "serving-settings")]
    let app = app.serving(serving_settings::from_environment()?);
    match role {
        Role::Web => {
            heard::learn(&db, worker).await?;
            app.serve(bind(settings.address).await?, stop_signal()?)
                .await
        }
        Role::Worker => {
            let worker = packages.worker(settings, db, worker).await?;
            worker.run(stop_signal()?).await
        }
        Role::All => {
            serve_both(
                app,
                packages.worker(settings, db, worker).await?,
                settings.address,
            )
            .await
        }
    }
}

async fn serve_both(app: App, worker: Worker, address: SocketAddr) -> Result<()> {
    let listener = bind(address).await?;
    let together = together::Together::new(stop_signal()?);
    let worker = tokio::spawn(worker.run(together.stopped()));
    let web = Box::pin(app.serve(listener, together.stopped()));
    together.run(web, worker).await
}

async fn bind(address: SocketAddr) -> Result<TcpListener> {
    TcpListener::bind(address)
        .await
        .map_err(|error| Error::internal(format!("binding {address}"), error))
}

/// Only an aggregate's own module, and its factory beneath it, can build or
/// change it:
///
/// ```
/// mod order {
///     use sofie::prelude::*;
///     #[derive(Aggregate)]
///     pub struct Order { meta: Meta, total: i64 }
///     pub mod factory {
///         pub fn place(total: i64) -> sofie::core::Result<super::Order> {
///             Ok(super::Order { meta: sofie::db::Meta::new()?, total })
///         }
///     }
///     impl Order {
///         pub fn double(&mut self) { self.total *= 2; }
///     }
/// }
/// let mut order = order::factory::place(1)?;
/// order.double();
/// # Ok::<(), sofie::core::Error>(())
/// ```
///
/// ```compile_fail,E0451
/// mod order {
///     use sofie::prelude::*;
///     #[derive(Aggregate)]
///     pub struct Order { meta: Meta, total: i64 }
/// }
/// let forged = order::Order { meta: sofie::db::Meta::new().unwrap(), total: 1 };
/// ```
///
/// ```compile_fail,E0616
/// mod order {
///     use sofie::prelude::*;
///     #[derive(Aggregate)]
///     pub struct Order { meta: Meta, total: i64 }
///     pub fn place() -> Order { Order { meta: sofie::db::Meta::new().unwrap(), total: 1 } }
/// }
/// let mut order = order::place();
/// order.total = 0;
/// ```
///
/// Only an aggregate's own crate checks its records across owners, as `across`
/// is crate-private:
///
/// ```
/// use sofie::prelude::*;
/// #[derive(Aggregate)]
/// pub struct Booking { meta: Meta, slot: i64 }
/// async fn taken(read: &Read) -> Result<bool> {
///     Booking::across(read).where_(Booking::SLOT.eq(10)).exists().await
/// }
/// ```
///
/// ```compile_fail,E0624
/// fn count_every_account(read: &sofie::db::Read) {
///     let _ = sofie_identity::Account::across(read);
/// }
/// ```
///
/// An aggregate has one owner at most, and it is an `Id`:
///
/// ```compile_fail
/// use sofie::prelude::*;
/// struct Member;
/// #[derive(Aggregate)]
/// pub struct Basket { meta: Meta, #[owner] a: Id<Member>, #[owner] b: Id<Member> }
/// ```
///
/// ```compile_fail
/// use sofie::prelude::*;
/// #[derive(Aggregate)]
/// pub struct Basket { meta: Meta, #[owner] member: String }
/// ```
///
/// A read model replies the states a field may move to as `<field>_to`, so
/// no field may take that name:
///
/// ```compile_fail
/// use sofie::prelude::*;
/// #[derive(Aggregate)]
/// pub struct Order { meta: Meta, status: i64, status_to: i64 }
/// #[derive(ReadModel)]
/// #[read(from = Order)]
/// pub struct Row { status: i64, status_to: i64 }
/// ```
///
/// A read model without `from` is filled by its loader, so it has no
/// aggregate to join through:
///
/// ```compile_fail
/// use sofie::prelude::*;
/// #[derive(Aggregate)]
/// pub struct Account { meta: Meta, name: String }
/// #[derive(ReadModel)]
/// pub struct Summary { #[read(via = Account::NAME, field = Account::NAME)] name: Option<String> }
/// ```
///
/// `like` matches text columns only, and only an aggregate that keeps its
/// deleted records can read them with `with_trashed`:
///
/// ```compile_fail
/// use sofie::prelude::*;
/// #[derive(Aggregate)]
/// pub struct Order { meta: Meta, total: i64 }
/// let _ = Order::query().where_(Order::TOTAL.like("1%"));
/// ```
///
/// ```compile_fail
/// use sofie::prelude::*;
/// #[derive(Aggregate)]
/// pub struct Order { meta: Meta, total: i64 }
/// let _ = Order::query().with_trashed();
/// ```
///
/// A GET, a screen or an island never writes: its handler takes only
/// read-side arguments and returns a reply, which is never committed.
///
/// ```
/// # use sofie::prelude::*;
/// # #[derive(Aggregate)]
/// # pub struct Order { meta: Meta, total: i64 }
/// # #[derive(ReadModel)]
/// # #[read(from = Order)]
/// # pub struct OrderRow { id: Id<Order>, total: i64 }
/// async fn show(order: OrderRow, _read: Read, _clock: Clock) -> Result<OrderRow> { Ok(order) }
/// async fn place(order: Order, _jobs: Jobs) -> Result<Order> { Ok(order) }
/// fn routes(routes: &mut Routes<Web>) {
///     routes.get("/orders/{order}", show);
///     routes.screen("/orders/{order}/page", "order", show);
///     routes.post("/orders/{order}", place);
/// }
/// ```
///
/// ```compile_fail,E0277
/// # use sofie::prelude::*;
/// # #[derive(Aggregate)]
/// # pub struct Order { meta: Meta, total: i64 }
/// # #[derive(ReadModel)]
/// # #[read(from = Order)]
/// # pub struct OrderRow { id: Id<Order>, total: i64 }
/// async fn show(order: Order) -> Result<()> { Ok(()) }
/// fn routes(routes: &mut Routes<Web>) { routes.get("/orders/{order}", show); }
/// ```
///
/// ```compile_fail,E0277
/// # use sofie::prelude::*;
/// async fn show(jobs: Jobs) -> Result<()> { Ok(()) }
/// fn routes(routes: &mut Routes<Web>) { routes.get("/orders/{order}", show); }
/// ```
///
/// ```compile_fail,E0277
/// # use sofie::prelude::*;
/// async fn show(session: Session) -> Result<()> { Ok(()) }
/// fn routes(routes: &mut Routes<Web>) { routes.get("/orders/{order}", show); }
/// ```
///
/// ```compile_fail,E0277
/// # use sofie::prelude::*;
/// async fn show(key: sofie::db::IdempotencyKey) -> Result<()> { Ok(()) }
/// fn routes(routes: &mut Routes<Web>) { routes.get("/orders/{order}", show); }
/// ```
///
/// ```compile_fail,E0277
/// # use sofie::prelude::*;
/// async fn show(flags: sofie::flags::Flags) -> Result<()> { Ok(()) }
/// fn routes(routes: &mut Routes<Web>) { routes.get("/orders/{order}", show); }
/// ```
///
/// ```compile_fail,E0277
/// # use sofie::prelude::*;
/// async fn show(blocks: sofie::http::ShieldBlocks) -> Result<()> { Ok(()) }
/// fn routes(routes: &mut Routes<Web>) { routes.get("/orders/{order}", show); }
/// ```
///
/// ```compile_fail,E0277
/// # use sofie::prelude::*;
/// async fn show(sign_ins: sofie::http::SignIns) -> Result<()> { Ok(()) }
/// fn routes(routes: &mut Routes<Web>) { routes.get("/orders/{order}", show); }
/// ```
///
/// ```compile_fail,E0277
/// # use sofie::prelude::*;
/// #[derive(Input)]
/// pub struct Note { text: String }
/// async fn show(note: Note) -> Result<()> { Ok(()) }
/// fn routes(routes: &mut Routes<Web>) { routes.get("/notes", show); }
/// ```
///
/// ```compile_fail,E0277
/// # use sofie::prelude::*;
/// trait Gateway: Send + Sync {}
/// async fn show(gateway: Dep<dyn Gateway>) -> Result<()> { Ok(()) }
/// fn routes(routes: &mut Routes<Web>) { routes.get("/orders", show); }
/// ```
///
/// ```compile_fail,E0277
/// # use sofie::prelude::*;
/// async fn show(jobs: Jobs) -> Result<()> { Ok(()) }
/// fn routes(routes: &mut Routes<Web>) { routes.screen("/orders", "orders", show); }
/// ```
///
/// ```compile_fail,E0277
/// # use sofie::prelude::*;
/// # #[derive(Aggregate)]
/// # pub struct Order { meta: Meta, total: i64 }
/// # #[derive(ReadModel)]
/// # #[read(from = Order)]
/// # pub struct OrderRow { id: Id<Order>, total: i64 }
/// async fn latest(read: Read) -> Result<Order> {
///     Order::query().first(&read).await?.ok_or(Error::NotFound)
/// }
/// fn routes(routes: &mut Routes<Web>) { routes.get("/orders/latest", latest); }
/// ```
///
/// ```compile_fail,E0277
/// # use sofie::prelude::*;
/// async fn visit() -> Result<sofie::db::Queued> { todo!() }
/// fn routes(routes: &mut Routes<Web>) { routes.get("/visit", visit); }
/// ```
pub mod prelude {
    #[cfg(feature = "tasks")]
    pub use crate::task;
    pub use crate::{Aggregate, Enum, Event, Input, ReadModel, Value, every, job, on};
    pub use sofie_core::Variants as _;
    pub use sofie_core::{Clock, Error, Instant, Result};
    pub use sofie_db::Aggregate as _;
    pub use sofie_db::{Id, Load, Meta, Read, Select, Timestamped};
    #[cfg(feature = "soft-delete")]
    pub use sofie_db::{SoftDelete, SoftDeletes};
    pub use sofie_http::{Access, App, Area, Principal, Routes, Session, Web};
    pub use sofie_values::{Currency, Email, Money};

    #[cfg(feature = "children")]
    pub use crate::Child;
    #[cfg(feature = "contracts")]
    pub use crate::contract;
    #[cfg(feature = "children")]
    pub use sofie_db::Children;
    #[cfg(feature = "jobs")]
    pub use sofie_db::Jobs;
    #[cfg(feature = "retention")]
    pub use sofie_db::Prune;
    #[cfg(feature = "publishing")]
    pub use sofie_db::Publishes as _;
    #[cfg(feature = "search")]
    pub use sofie_db::Searches as _;
    #[cfg(feature = "flags")]
    pub use sofie_db::{Activates as _, Defaults as _};
    #[cfg(feature = "history")]
    pub use sofie_db::{Authored as _, History as _};
    #[cfg(feature = "codes")]
    pub use sofie_db::{Coded as _, Slugs as _};
    #[cfg(feature = "contracts")]
    pub use sofie_db::{Dep, Extensions, Lookup};
    #[cfg(feature = "local-time")]
    pub use sofie_db::{LocalJobs, Repeat};
    #[cfg(feature = "pagination")]
    pub use sofie_db::{Page, Paginated};
    #[cfg(feature = "groups")]
    pub use sofie_http::Groups;
    #[cfg(feature = "resources")]
    pub use sofie_http::Resources;
    #[cfg(feature = "menu")]
    pub use sofie_http::{Menu, Navigation};
    #[cfg(feature = "mail")]
    pub use sofie_mail::Mailer;
    #[cfg(feature = "notify")]
    pub use sofie_notify::{Inbox, Notify, Preferences};
    #[cfg(feature = "time")]
    pub use sofie_values::Date;
    #[cfg(feature = "person")]
    pub use sofie_values::PersonName;
    #[cfg(feature = "web")]
    pub use sofie_values::Url;
}
