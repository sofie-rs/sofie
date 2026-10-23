use std::collections::BTreeMap;

use serde_json::{Map, Value};
use sofie_core::{Error, Result};
use sofie_db::{Call, Context, Db, Outcome, Reach, Run, Writes, erase};
use tracing::field::Empty;
use tracing::{Instrument, Span};

/// What `<app> task NAME` may run; `sofie gen` writes the app's as `tasks()`.
#[derive(Default)]
pub struct Tasks {
    registered: BTreeMap<&'static str, Run<TaskContext>>,
}

impl Tasks {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Panics when a task named `name` is already registered.
    #[must_use]
    pub fn task<Args: 'static>(
        mut self,
        name: &'static str,
        task: impl Call<TaskContext, Args>,
    ) -> Self {
        assert!(
            self.registered.insert(name, erase(task)).is_none(),
            "task `{name}` is registered twice"
        );
        self
    }

    /// Its `#[derive(Input)]` argument reads `arguments`, validated as a request's body is;
    /// what it returns is committed as a job's outcome is.
    pub async fn run(&self, db: &Db, name: &str, arguments: Map<String, Value>) -> Result<()> {
        let task = self.find(name)?;
        let mut context = TaskContext {
            db: db.clone(),
            arguments: Some(arguments),
        };
        let span = task_span(name);
        let work = async move {
            let mut outcome = task(&mut context).await?;
            context.finish(&mut *outcome).await
        };
        let outcome = work.instrument(span.clone()).await;
        if let Err(error) = &outcome {
            span.record("otel.status_code", "ERROR");
            span.in_scope(|| {
                tracing::error!(error.kind = "task_failed", error = %error, task = name, "a task failed");
            });
        }
        outcome
    }

    fn find(&self, name: &str) -> Result<&Run<TaskContext>> {
        self.registered.get(name).ok_or_else(|| {
            let known: Vec<&str> = self.registered.keys().copied().collect();
            Error::internal_message(format!(
                "the app has no task `{name}`; its tasks: {}",
                if known.is_empty() {
                    "none".to_owned()
                } else {
                    known.join(", ")
                }
            ))
        })
    }
}

/// A task acts for no principal, so it reaches every record.
pub struct TaskContext {
    db: Db,
    arguments: Option<Map<String, Value>>,
}

impl TaskContext {
    async fn finish(self, outcome: &mut dyn Outcome) -> Result<()> {
        if let Some(unread) = self.arguments.filter(|arguments| !arguments.is_empty()) {
            let names: Vec<String> = unread.keys().map(|name| format!("--{name}")).collect();
            return Err(Error::BadRequest(format!(
                "the task takes no arguments, so it refuses {}",
                names.join(" ")
            )));
        }
        self.db.commit(outcome).await
    }
}

impl Context for TaskContext {
    fn db(&self) -> &Db {
        &self.db
    }

    fn reach(&self) -> Reach {
        Reach::All
    }

    fn input(&mut self) -> Result<Value> {
        self.arguments.take().map(Value::Object).ok_or_else(|| {
            Error::internal_message(
                "a task's arguments are read by one `#[derive(Input)]` argument",
            )
        })
    }
}

impl Writes for TaskContext {}

fn task_span(name: &str) -> Span {
    let span = tracing::info_span!(
        "task",
        otel.name = format!("task {name}").as_str(),
        otel.status_code = Empty,
        module = name.split('.').next(),
        trace_id = Empty,
    );
    let _ = sofie_telemetry::continue_trace(&span, None);
    span
}

/// Builds the app, for its configs and contracts, and the worker, for the events it hears, only
/// once the task is known.
pub(crate) async fn run_by_hand(
    tasks: Option<fn() -> Tasks>,
    db: Db,
    name: &str,
    arguments: Map<String, Value>,
    app: crate::AppBuilder<'_>,
    worker: crate::WorkerBuilder<'_>,
) -> Result<()> {
    let wired = tasks.is_some();
    let tasks = framework_tasks(tasks.map_or_else(Tasks::new, |tasks| tasks()));
    if let Err(missing) = tasks.find(name) {
        return Err(if wired {
            missing
        } else {
            Error::internal_message(
                "the app runs no tasks: call `sofie::run_with_tasks` from `main` with the wiring's `tasks`",
            )
        });
    }
    app(db.clone()).await?;
    crate::heard::learn(&db, worker).await?;
    tasks.run(&db, name, arguments).await
}

/// The framework's own, beside the app's.
fn framework_tasks(tasks: Tasks) -> Tasks {
    #[cfg(feature = "search-meilisearch")]
    return tasks.task(sofie_db::REINDEX_TASK, sofie_db::reindex);
    #[cfg(not(feature = "search-meilisearch"))]
    tasks
}

/// `--name value` pairs, as `#[derive(Input)]` reads a form: every value a string.
pub(crate) fn named_arguments(words: &[&str]) -> Result<Map<String, Value>> {
    let mut arguments = Map::new();
    for pair in words.chunks(2) {
        let [flag, value] = pair else {
            return Err(refused(&format!("`{}` has no value", pair[0])));
        };
        let name = flag
            .strip_prefix("--")
            .filter(|name| is_argument_name(name))
            .ok_or_else(|| refused(&format!("`{flag}` is not `--name`")))?;
        if arguments
            .insert(name.to_owned(), Value::String((*value).to_owned()))
            .is_some()
        {
            return Err(refused(&format!("`{flag}` is given twice")));
        }
    }
    Ok(arguments)
}

fn is_argument_name(name: &str) -> bool {
    name.starts_with(|first: char| first.is_ascii_lowercase())
        && name.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

fn refused(problem: &str) -> Error {
    Error::BadRequest(format!(
        "{problem}; a task's arguments are `--name value` pairs, named as its input's fields"
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use sofie_http::App;
    use sofie_postgres::Postgres;

    use super::*;

    static BUILT: AtomicBool = AtomicBool::new(false);
    static RAN: AtomicBool = AtomicBool::new(false);

    async fn noted() -> Result<()> {
        assert!(BUILT.load(Ordering::SeqCst), "the app is built first");
        RAN.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn registered() -> Tasks {
        Tasks::new().task("shop.noted", noted)
    }

    async fn never_built(_db: Db) -> Result<App> {
        panic!("the app is built only for a known task")
    }

    fn never_hearing(_db: Db) -> sofie_db::worker::Worker {
        panic!("the worker is built only for a known task")
    }

    async fn built(db: Db) -> Result<App> {
        BUILT.store(true, Ordering::SeqCst);
        Ok(App::new(db))
    }

    #[tokio::test]
    async fn by_hand_a_task_is_found_before_the_app_and_worker_are_built_and_runs_after() {
        let db = Db::new(Postgres::connect("postgres://sofie@127.0.0.1:1/unused").unwrap());

        let unwired = run_by_hand(
            None,
            db.clone(),
            "shop.noted",
            Map::new(),
            crate::app_builder(never_built),
            crate::worker_builder(never_hearing),
        )
        .await
        .unwrap_err();
        let unknown = run_by_hand(
            Some(registered),
            db.clone(),
            "shop.other",
            Map::new(),
            crate::app_builder(never_built),
            crate::worker_builder(never_hearing),
        )
        .await
        .unwrap_err();
        run_by_hand(
            Some(registered),
            db,
            "shop.noted",
            Map::new(),
            crate::app_builder(built),
            crate::worker_builder(sofie_db::worker::Worker::new),
        )
        .await
        .unwrap();

        assert!(
            unwired.to_string().contains("`sofie::run_with_tasks`"),
            "{unwired}"
        );
        assert!(
            unknown.to_string().contains("its tasks: shop.noted"),
            "{unknown}"
        );
        assert!(RAN.load(Ordering::SeqCst));
    }

    #[test]
    fn arguments_are_named_pairs_of_strings() {
        let arguments = named_arguments(&["--since", "2026-01-01", "--limit", "-1"]).unwrap();
        assert_eq!(
            serde_json::Value::Object(arguments),
            serde_json::json!({ "since": "2026-01-01", "limit": "-1" })
        );
        assert!(named_arguments(&[]).unwrap().is_empty());
    }

    #[test]
    fn a_lone_flag_a_bare_word_or_a_repeated_name_is_refused() {
        for words in [
            &["--since"][..],
            &["since", "x"],
            &["--", "x"],
            &["--Since", "x"],
            &["--since", "x", "--since", "y"],
        ] {
            let error = named_arguments(words).unwrap_err();
            assert!(
                error.to_string().contains("`--name value` pairs"),
                "{error}"
            );
        }
    }
}
