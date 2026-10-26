#![cfg(feature = "product-reports")]

use sofie_core::{Date, Result, SqlValue, Timestamp, Uuid};
use sofie_db::worker::Worker;
use sofie_db::{
    Cohort, Db, Outcome, Period, Plan, ProductEvents, Prune, Retention, cohorts, forget_principal,
    query_values,
};
use sofie_postgres::Postgres;
use sofie_testing::TestDatabase;

struct Migrated {
    _database: TestDatabase,
    db: Db,
    postgres: Postgres,
}

async fn migrated() -> Migrated {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema().migrate(&[]).await.unwrap();
    let postgres = Postgres::connect(database.url()).unwrap();
    Migrated {
        _database: database,
        db,
        postgres,
    }
}

struct Forgetting(Uuid);

impl Outcome for Forgetting {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        forget_principal(plan, self.0);
        Ok(())
    }
}

async fn forget(db: &Db, principal: u128) {
    db.commit(&mut Forgetting(Uuid::from_u128(principal)))
        .await
        .unwrap();
}

async fn insert(postgres: &Postgres, at: &str, name: &str, principal: u128) {
    query_values(
        postgres,
        "INSERT INTO sofie.product_events (at, name, principal) VALUES ($1, $2, $3)",
        &[
            SqlValue::Timestamp(at.parse().unwrap()),
            SqlValue::Text(name.to_owned()),
            SqlValue::Uuid(Uuid::from_u128(principal)),
        ],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn events_raised_in_a_span_are_written_with_their_principal_unless_it_was_forgotten() {
    let migrated = migrated().await;
    let (db, postgres) = (&migrated.db, &migrated.postgres);
    let _capture = sofie_telemetry::capture();
    let kept = ProductEvents::keep(db).unwrap();
    let request = tracing::info_span!("request");
    sofie_telemetry::attach_principal(&request, Uuid::from_u128(1));
    let closing = tracing::info_span!("closing");
    sofie_telemetry::attach_principal(&closing, Uuid::from_u128(3));
    closing.in_scope(|| sofie_telemetry::product_event!(account.closed));
    forget(db, 3).await;

    request.in_scope(|| sofie_telemetry::product_event!(account.signed_up));
    sofie_telemetry::product_event!(page.viewed);
    kept.flush().await;

    let rows = query_values(
        postgres,
        "SELECT name, principal FROM sofie.product_events ORDER BY name",
        &[],
    )
    .await
    .unwrap();
    assert_eq!(
        rows,
        [
            vec![
                SqlValue::Text("account.signed_up".to_owned()),
                SqlValue::Uuid(Uuid::from_u128(1))
            ],
            vec![SqlValue::Text("page.viewed".to_owned()), SqlValue::Null],
        ]
    );
    assert!(ProductEvents::keep(db).is_err(), "kept once per process");
}

#[tokio::test]
async fn cohorts_count_who_returned_in_each_period_after_their_first() {
    let migrated = migrated().await;
    let (db, postgres) = (&migrated.db, &migrated.postgres);
    // Weeks start on Monday: 2026-09-07 and 2026-09-14.
    insert(postgres, "2026-09-08T10:00:00Z", "signed_up", 1).await;
    insert(postgres, "2026-09-09T10:00:00Z", "signed_up", 2).await;
    insert(postgres, "2026-09-15T10:00:00Z", "signed_up", 3).await;
    insert(postgres, "2026-09-10T10:00:00Z", "ordered", 1).await;
    insert(postgres, "2026-09-16T10:00:00Z", "ordered", 1).await;
    insert(postgres, "2026-09-17T10:00:00Z", "ordered", 2).await;
    insert(postgres, "2026-09-29T10:00:00Z", "ordered", 3).await;
    insert(postgres, "2026-09-01T10:00:00Z", "signed_up", 4).await;

    let weekly = cohorts(
        db,
        &Retention {
            start: "signed_up",
            returning: "ordered",
            period: Period::Week,
            since: "2026-09-07T00:00:00Z".parse::<Timestamp>().unwrap(),
        },
    )
    .await
    .unwrap();
    let monthly = cohorts(
        db,
        &Retention {
            start: "signed_up",
            returning: "ordered",
            period: Period::Month,
            since: "2026-09-07T00:00:00Z".parse::<Timestamp>().unwrap(),
        },
    )
    .await
    .unwrap();

    let date = |text: &str| text.parse::<Date>().unwrap();
    assert_eq!(
        weekly,
        [
            Cohort {
                start: date("2026-09-07"),
                size: 2,
                returned: vec![1, 2],
            },
            Cohort {
                start: date("2026-09-14"),
                size: 1,
                returned: vec![0, 0, 1],
            },
        ]
    );
    assert_eq!(
        monthly,
        [Cohort {
            start: date("2026-09-01"),
            size: 3,
            returned: vec![3],
        }]
    );
}

#[tokio::test]
async fn events_past_their_retention_are_pruned() {
    let migrated = migrated().await;
    let (db, postgres) = (&migrated.db, &migrated.postgres);
    insert(postgres, "2026-01-10T10:00:00Z", "signed_up", 1).await;
    insert(postgres, "2026-09-10T10:00:00Z", "signed_up", 2).await;

    let ran = Worker::new(db.clone())
        .every(
            "sofie.product_events.prune",
            std::time::Duration::from_hours(1),
            |prune: Prune| async move {
                let before = sofie_core::Instant::parse("2026-06-01T00:00:00Z")?;
                prune.product_events(before).await.map(drop)
            },
        )
        .drain()
        .await
        .unwrap();

    let left = query_values(postgres, "SELECT principal FROM sofie.product_events", &[])
        .await
        .unwrap();
    assert_eq!(ran, 1);
    assert_eq!(left, [vec![SqlValue::Uuid(Uuid::from_u128(2))]]);
}

#[tokio::test]
async fn a_closed_account_s_events_are_forgotten() {
    let migrated = migrated().await;
    let (db, postgres) = (&migrated.db, &migrated.postgres);
    insert(postgres, "2026-09-10T10:00:00Z", "signed_up", 1).await;
    insert(postgres, "2026-09-11T10:00:00Z", "page.viewed", 1).await;
    insert(postgres, "2026-09-12T10:00:00Z", "signed_up", 2).await;

    forget(db, 1).await;

    let left = query_values(postgres, "SELECT principal FROM sofie.product_events", &[])
        .await
        .unwrap();
    assert_eq!(left, [vec![SqlValue::Uuid(Uuid::from_u128(2))]]);
}

#[tokio::test]
async fn an_event_a_write_racing_the_erasure_kept_is_swept_by_the_prune() {
    let migrated = migrated().await;
    let (db, postgres) = (&migrated.db, &migrated.postgres);
    forget(db, 1).await;
    insert(postgres, "2026-09-10T10:00:00Z", "page.viewed", 1).await;
    insert(postgres, "2026-09-10T10:00:00Z", "page.viewed", 2).await;

    Worker::new(db.clone())
        .every(
            "sofie.product_events.prune",
            std::time::Duration::from_hours(1),
            |prune: Prune| async move {
                let before = sofie_core::Instant::parse("2026-06-01T00:00:00Z")?;
                prune.product_events(before).await.map(drop)
            },
        )
        .drain()
        .await
        .unwrap();

    let left = query_values(postgres, "SELECT principal FROM sofie.product_events", &[])
        .await
        .unwrap();
    assert_eq!(left, [vec![SqlValue::Uuid(Uuid::from_u128(2))]]);
}

#[tokio::test]
async fn the_forgotten_principals_find_their_events_by_an_index() {
    let migrated = migrated().await;

    let indexed = query_values(
        &migrated.postgres,
        "SELECT indexdef FROM pg_indexes
        WHERE schemaname = 'sofie' AND indexname = 'product_events_principal'",
        &[],
    )
    .await
    .unwrap();

    assert_eq!(
        indexed,
        [[SqlValue::Text(
            "CREATE INDEX product_events_principal ON sofie.product_events USING btree (principal)"
                .to_owned()
        )]]
    );
}

#[tokio::test]
async fn the_sweep_finds_the_last_day_s_forgotten_principals_by_an_index() {
    let migrated = migrated().await;

    let indexed = query_values(
        &migrated.postgres,
        "SELECT indexdef FROM pg_indexes
        WHERE schemaname = 'sofie' AND indexname = 'forgotten_principals_at'",
        &[],
    )
    .await
    .unwrap();

    assert_eq!(
        indexed,
        [[SqlValue::Text(
            "CREATE INDEX forgotten_principals_at ON sofie.forgotten_principals \
             USING btree (forgotten_at)"
                .to_owned()
        )]]
    );
}
