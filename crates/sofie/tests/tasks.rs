#![cfg(all(feature = "tasks", feature = "capture"))]

use sofie::core::SqlValue;
use sofie::db::{Db, File, Migrations};
use sofie::postgres::Postgres;
use sofie::prelude::*;
use sofie::serde_json::{Map, Value, json};
use sofie::{Tasks, task};
use sofie_testing::TestDatabase;

const TABLES: &str = "
CREATE TABLE counter (
    id uuid PRIMARY KEY, version bigint NOT NULL,
    created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
    count bigint NOT NULL
);
";

#[derive(Aggregate)]
pub struct Counter {
    meta: Meta,
    count: i64,
}

#[derive(Input)]
pub struct Step {
    #[min(1)]
    by: i64,
}

#[task]
async fn bump(read: Read, _clock: Clock, step: Step) -> Result<Vec<Counter>> {
    let mut counters = Counter::query().all(&read).await?;
    for counter in &mut counters {
        counter.count += step.by;
    }
    Ok(counters)
}

#[task]
async fn reset(read: Read) -> Result<Vec<Counter>> {
    let mut counters = Counter::query().all(&read).await?;
    for counter in &mut counters {
        counter.count = 0;
    }
    Ok(counters)
}

#[task]
async fn refuse() -> Result<()> {
    Err(Error::Rule(
        "nothing to backfill before the cut-over".to_owned(),
    ))
}

fn tasks() -> Tasks {
    Tasks::new()
        .task("tasks.bump", bump)
        .task("tasks.reset", reset)
        .task("tasks.refuse", refuse)
}

struct Setup {
    db: Db,
    raw: Postgres,
    _database: TestDatabase,
}

impl Setup {
    async fn new() -> Self {
        let database = TestDatabase::create().await;
        let db = Db::new(Postgres::connect(database.url()).unwrap());
        db.schema()
            .migrate(&[Migrations::new(
                "tasks",
                const { &[File::new("0001_tables.sql", TABLES.as_bytes())] },
            )])
            .await
            .unwrap();
        let mut counter = Counter {
            meta: Meta::new().unwrap(),
            count: 1,
        };
        db.commit(&mut counter).await.unwrap();
        Self {
            db,
            raw: Postgres::connect(database.url()).unwrap(),
            _database: database,
        }
    }

    async fn run(&self, name: &str, arguments: Value) -> Result<()> {
        let Value::Object(arguments) = arguments else {
            panic!("arguments are an object");
        };
        tasks().run(&self.db, name, arguments).await
    }

    async fn count(&self) -> Vec<Vec<SqlValue>> {
        sofie::db::query_values(&self.raw, "SELECT count FROM tasks.counter", &[])
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn a_task_reads_its_arguments_as_an_input_and_commits_what_it_returns() {
    let setup = Setup::new().await;
    let telemetry = sofie::telemetry::capture();

    setup.run("tasks.bump", json!({ "by": "2" })).await.unwrap();

    assert_eq!(setup.count().await, [[SqlValue::Int(3)]]);
    let span = telemetry
        .spans()
        .into_iter()
        .find(|span| span.name == "task")
        .expect("the task ran in a span");
    assert_eq!(span.fields["otel.name"], "task tasks.bump");
    assert_eq!(span.fields["module"], "tasks");
}

#[tokio::test]
async fn arguments_are_validated_before_the_task_runs() {
    let setup = Setup::new().await;

    for (arguments, expected) in [
        (json!({ "by": "0" }), "by: must be at least 1"),
        (json!({}), "by: is required"),
        (json!({ "by": "two" }), "by: "),
        (json!({ "by": "1", "times": "3" }), "times: is not accepted"),
    ] {
        let error = setup.run("tasks.bump", arguments).await.unwrap_err();
        assert!(
            matches!(error, Error::Invalid(_)) && error.to_string().contains(expected),
            "{error}"
        );
    }
    assert_eq!(setup.count().await, [[SqlValue::Int(1)]]);
}

#[tokio::test]
async fn a_task_without_an_input_refuses_arguments_and_commits_nothing() {
    let setup = Setup::new().await;

    let error = setup
        .run("tasks.reset", json!({ "by": "1" }))
        .await
        .unwrap_err();

    assert!(error.to_string().contains("refuses --by"), "{error}");
    assert_eq!(setup.count().await, [[SqlValue::Int(1)]]);
    setup.run("tasks.reset", json!({})).await.unwrap();
    assert_eq!(setup.count().await, [[SqlValue::Int(0)]]);
}

#[tokio::test]
async fn a_failing_task_answers_its_error_and_logs_it_in_its_span() {
    let setup = Setup::new().await;
    let telemetry = sofie::telemetry::capture();

    let error = setup.run("tasks.refuse", json!({})).await.unwrap_err();

    assert!(matches!(error, Error::Rule(_)), "{error}");
    let span = telemetry
        .spans()
        .into_iter()
        .find(|span| span.name == "task")
        .unwrap();
    assert_eq!(span.fields["otel.status_code"], "ERROR");
    assert!(
        telemetry
            .events()
            .iter()
            .any(|event| event.fields.get("error.kind").map(String::as_str) == Some("task_failed")),
        "{:?}",
        telemetry.events()
    );
}

#[tokio::test]
async fn an_unknown_task_names_the_known_ones() {
    let setup = Setup::new().await;

    let error = tasks()
        .run(&setup.db, "tasks.backfill", Map::new())
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "the app has no task `tasks.backfill`; its tasks: tasks.bump, tasks.refuse, tasks.reset"
    );
}

#[test]
#[should_panic(expected = "task `tasks.bump` is registered twice")]
fn a_name_is_registered_once() {
    let _ = tasks().task("tasks.bump", reset);
}
