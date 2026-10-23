use sofie_core::{Cast, ColumnReader, Error, Result, SqlValue};
use sofie_db::{Aggregate, Column, Db, Driver, ExpectedRows, Field, Id, Load, Meta, Select, Step};
use sofie_postgres::Postgres;
use sofie_testing::TestDatabase;

const CREATE_ORDERS: &str = "CREATE SCHEMA shop;
CREATE TABLE shop.orders (
    id uuid PRIMARY KEY,
    version int8 NOT NULL,
    member text NOT NULL,
    total int8 NOT NULL,
    note text
)";

struct Order {
    meta: Meta,
    member: String,
    total: i64,
    note: Option<String>,
}

impl Order {
    const MEMBER: Column<Self, String> = Column::new("member");
    const TOTAL: Column<Self, i64> = Column::new("total");
    const NOTE: Column<Self, Option<String>> = Column::new("note");

    fn new(member: &str, total: i64) -> Self {
        Self {
            meta: Meta::new().unwrap(),
            member: member.to_owned(),
            total,
            note: None,
        }
    }
}

impl Aggregate for Order {
    const NAME: &'static str = "orders";
    const MODULE: &'static str = "shop";
    const FIELDS: &'static [Field] = &[
        Field::new("member", String::COLUMNS, false),
        Field::new("total", i64::COLUMNS, false),
        Field::new("note", <Option<String>>::COLUMNS, true),
    ];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.member.encode(out);
        self.total.encode(out);
        self.note.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            member: String::decode(row)?,
            total: i64::decode(row)?,
            note: Option::decode(row)?,
        })
    }
}

async fn shop() -> (TestDatabase, Db, Postgres) {
    let database = TestDatabase::create().await;
    let outside = Postgres::connect(database.url()).unwrap();
    outside
        .transaction(vec![Step::Script(CREATE_ORDERS.to_owned())])
        .await
        .unwrap();
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    (database, db, outside)
}

async fn committed(db: &Db, member: &str, total: i64) -> Order {
    let mut order = Order::new(member, total);
    db.commit(&mut order).await.unwrap();
    order
}

async fn write_outside(outside: &Postgres, sql: &str, id: Id<Order>) {
    outside
        .transaction(vec![Step::Statement {
            sql: sql.to_owned(),
            params: vec![SqlValue::Uuid(id.uuid())],
            expect_rows: Some(ExpectedRows::or_conflict(1)),
        }])
        .await
        .unwrap();
}

#[tokio::test]
async fn a_committed_aggregate_loads_back_whole() {
    let (_database, db, _outside) = shop().await;
    let order = committed(&db, "ada", 120).await;

    let loaded = Order::find(&db.read(), order.id()).await.unwrap();

    assert_eq!(loaded.id(), order.id());
    assert_eq!(loaded.meta().version(), 1);
    assert_eq!(order.meta().version(), 1);
    assert_eq!(
        (loaded.member.as_str(), loaded.total, loaded.note),
        ("ada", 120, None)
    );
}

#[tokio::test]
async fn an_unknown_id_is_not_found() {
    let (_database, db, _outside) = shop().await;

    let result = Order::find(
        &db.read(),
        Id::from_uuid(sofie_core::Uuid::now_v7().unwrap()),
    )
    .await;

    assert!(matches!(result, Err(Error::NotFound)));
}

#[tokio::test]
async fn an_update_writes_the_loaded_record_whole_and_bumps_the_version() {
    let (_database, db, outside) = shop().await;
    let order = committed(&db, "ada", 120).await;
    let mut loaded = Order::find(&db.read(), order.id()).await.unwrap();
    write_outside(
        &outside,
        "UPDATE shop.orders SET member = 'renamed outside' WHERE id = $1",
        order.id(),
    )
    .await;

    loaded.total = 150;
    db.commit(&mut loaded).await.unwrap();

    let reloaded = Order::find(&db.read(), order.id()).await.unwrap();
    assert_eq!(reloaded.member, "ada");
    assert_eq!(reloaded.total, 150);
    assert_eq!(reloaded.meta().version(), 2);
    assert_eq!(loaded.meta().version(), 2);
}

struct Reviewed;

impl sofie_db::Input for Reviewed {
    fn from_json(_value: serde_json::Value) -> Result<Self> {
        Ok(Self)
    }
}

impl serde::Serialize for Reviewed {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_unit_struct("Reviewed")
    }
}

impl sofie_db::Event for Reviewed {
    const NAME: &'static str = "shop.Reviewed";
}

#[tokio::test]
async fn an_event_raised_from_a_stale_copy_is_a_conflict() {
    let (_database, db, outside) = shop().await;
    db.schema().migrate(&[]).await.unwrap();
    let order = committed(&db, "ada", 120).await;
    let mut first = Order::find(&db.read(), order.id()).await.unwrap();
    let mut stale = Order::find(&db.read(), order.id()).await.unwrap();

    first.meta_mut().raise(Reviewed).unwrap();
    db.commit(&mut first).await.unwrap();
    stale.meta_mut().raise(Reviewed).unwrap();
    let result = db.commit(&mut stale).await;

    assert!(matches!(result, Err(Error::Conflict)), "{result:?}");
    assert_eq!(first.meta().version(), 2);
    let reloaded = Order::find(&db.read(), order.id()).await.unwrap();
    assert_eq!(reloaded.meta().version(), 2);
    let events = sofie_db::query_values(
        &outside,
        "SELECT id FROM sofie.messages WHERE name = 'shop.Reviewed'",
        &[],
    )
    .await
    .unwrap();
    assert_eq!(events.len(), 1);
}

#[tokio::test]
async fn committing_an_unchanged_aggregate_writes_nothing() {
    let (_database, db, outside) = shop().await;
    let order = committed(&db, "ada", 120).await;
    let mut loaded = Order::find(&db.read(), order.id()).await.unwrap();
    write_outside(
        &outside,
        "UPDATE shop.orders SET version = 7 WHERE id = $1",
        order.id(),
    )
    .await;

    db.commit(&mut loaded).await.unwrap();

    assert_eq!(loaded.meta().version(), 1);
}

#[tokio::test]
async fn committing_a_stale_copy_is_a_conflict() {
    let (_database, db, _outside) = shop().await;
    let order = committed(&db, "ada", 120).await;
    let mut first = Order::find(&db.read(), order.id()).await.unwrap();
    let mut stale = Order::find(&db.read(), order.id()).await.unwrap();

    first.total = 150;
    db.commit(&mut first).await.unwrap();
    stale.note = Some("too late".to_owned());
    let result = db.commit(&mut stale).await;

    assert!(matches!(result, Err(Error::Conflict)), "{result:?}");
    let reloaded = Order::find(&db.read(), order.id()).await.unwrap();
    assert_eq!((reloaded.total, reloaded.note), (150, None));
}

#[tokio::test]
async fn a_nullable_column_compares_its_null_rows_too() {
    let (_database, db, _outside) = shop().await;
    let mut orders = vec![Order::new("ada", 10), Order::new("grace", 20)];
    orders[1].note = Some("gift".to_owned());
    db.commit(&mut orders).await.unwrap();
    let read = db.read();

    let without_note = Order::query().where_(Order::NOTE.eq(None)).all(&read);
    let not_a_gift = Order::query()
        .where_(Order::NOTE.ne(Some("gift".to_owned())))
        .all(&read);
    let with_note = Order::query().where_(Order::NOTE.ne(None)).all(&read);

    let members = |orders: Vec<Order>| -> Vec<String> {
        orders.into_iter().map(|order| order.member).collect()
    };
    assert_eq!(members(without_note.await.unwrap()), ["ada"]);
    assert_eq!(members(not_a_gift.await.unwrap()), ["ada"]);
    assert_eq!(members(with_note.await.unwrap()), ["grace"]);
}

#[tokio::test]
async fn a_count_and_an_existence_check_ignore_the_limit() {
    let (_database, db, _outside) = shop().await;
    let mut orders = vec![Order::new("ada", 10), Order::new("grace", 20)];
    db.commit(&mut orders).await.unwrap();
    let read = db.read();

    let counted = Order::query().limit(1).count(&read).await.unwrap();
    let exists = Order::query().limit(0).exists(&read).await.unwrap();

    assert_eq!(counted, 2);
    assert!(exists);
}

#[tokio::test]
async fn queries_filter_sort_limit_and_count() {
    let (_database, db, _outside) = shop().await;
    let mut orders = vec![
        Order::new("ada", 10),
        Order::new("grace", 20),
        Order::new("ada", 30),
    ];
    orders[1].note = Some("gift".to_owned());
    db.commit(&mut orders).await.unwrap();
    let read = db.read();

    let large = Order::query()
        .where_(Order::TOTAL.gt(15))
        .count(&read)
        .await;
    let by_ada = Order::query()
        .where_(Order::MEMBER.eq("ada".to_owned()))
        .count(&read)
        .await;
    let without_note = Order::query()
        .where_(Order::NOTE.is_null())
        .where_(Order::TOTAL.lt(30).or(Order::TOTAL.gte(30)))
        .count(&read)
        .await;
    let newest = Order::query().latest().limit(2).all(&read).await.unwrap();

    assert_eq!(large.unwrap(), 2);
    assert_eq!(by_ada.unwrap(), 2);
    assert_eq!(without_note.unwrap(), 2);
    let newest_totals: Vec<i64> = newest.iter().map(|order| order.total).collect();
    assert_eq!(newest_totals, vec![30, 20]);
}

#[tokio::test]
async fn queries_negate_bound_from_above_and_sort_by_a_column() {
    let (_database, db, _outside) = shop().await;
    let mut orders = vec![
        Order::new("ada", 10),
        Order::new("grace", 20),
        Order::new("ada", 30),
    ];
    orders[1].note = Some("gift".to_owned());
    db.commit(&mut orders).await.unwrap();
    let read = db.read();

    let not_ada = Order::query()
        .where_(Order::MEMBER.ne("ada".to_owned()))
        .count(&read)
        .await;
    let small_with_note = Order::query()
        .where_(Order::TOTAL.lte(20))
        .where_(Order::NOTE.is_not_null())
        .count(&read)
        .await;
    let descending = Order::query()
        .order_by(Order::TOTAL.desc())
        .all(&read)
        .await
        .unwrap();
    let cheapest = Order::query()
        .order_by(Order::TOTAL.asc())
        .limit(1)
        .all(&read)
        .await
        .unwrap();

    assert_eq!(not_ada.unwrap(), 1);
    assert_eq!(small_with_note.unwrap(), 1);
    let totals: Vec<i64> = descending.iter().map(|order| order.total).collect();
    assert_eq!(totals, vec![30, 20, 10]);
    assert_eq!(cheapest[0].total, 10);
}

#[tokio::test]
async fn the_schema_check_names_where_a_table_and_its_schema_disagree() {
    use sofie_core::SqlType;
    use sofie_db::{Mismatch, MismatchKind};

    let (_database, db, outside) = shop().await;
    assert_eq!(db.schema().mismatches::<Order>().await.unwrap(), []);

    outside
        .transaction(vec![Step::Script(
            "ALTER TABLE shop.orders DROP COLUMN total, ALTER COLUMN member TYPE int8 \
             USING 0, ALTER COLUMN note SET NOT NULL, ALTER COLUMN version DROP NOT NULL, \
             ADD COLUMN placed_at timestamptz NOT NULL, ADD COLUMN status text NOT NULL \
             DEFAULT 'NEW'"
                .to_owned(),
        )])
        .await
        .unwrap();

    let kinds: Vec<MismatchKind> = db
        .schema()
        .mismatches::<Order>()
        .await
        .unwrap()
        .into_iter()
        .map(|Mismatch { table, kind }| {
            assert_eq!(table, "shop.orders");
            kind
        })
        .collect();
    assert_eq!(
        kinds,
        [
            MismatchKind::Nullable {
                column: "version".to_owned()
            },
            MismatchKind::Type {
                column: "member".to_owned(),
                expected: SqlType::Text,
                found: "int8".to_owned()
            },
            MismatchKind::MissingColumn {
                column: "total".to_owned(),
                expected: SqlType::Int
            },
            MismatchKind::NotNull {
                column: "note".to_owned()
            },
            MismatchKind::Unwritten {
                column: "placed_at".to_owned()
            },
        ]
    );
}
