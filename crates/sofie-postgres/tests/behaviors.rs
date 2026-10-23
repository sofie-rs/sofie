use sofie_core::{Cast, ColumnReader, Error, Result, SqlValue, Uuid};
use sofie_db::{
    Activates, Aggregate, Authored, Behavior, ChangeKind, Coded, Db, Defaults, Driver, Field,
    History, Load, Meta, Publishes, Reach, Read, Select, Slugged, Slugs, SoftDeletes, Step, Uses,
    active,
};
use sofie_postgres::Postgres;
use sofie_testing::TestDatabase;

const CREATE_PRODUCTS: &str = "CREATE SCHEMA catalog;
CREATE TABLE catalog.product (
    id uuid PRIMARY KEY,
    version int8 NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    deleted_at timestamptz,
    is_active bool NOT NULL,
    is_default bool NOT NULL,
    slug text NOT NULL,
    code text NOT NULL UNIQUE,
    created_by uuid,
    updated_by uuid,
    published_at timestamptz,
    shop uuid NOT NULL,
    name text NOT NULL
)";

struct Product {
    meta: Meta,
    shop: Uuid,
    name: String,
}

impl Product {
    fn new(shop: Uuid, name: &str) -> Self {
        Self {
            meta: Meta::new().unwrap(),
            shop,
            name: name.to_owned(),
        }
    }
}

impl Aggregate for Product {
    const NAME: &'static str = "product";
    const MODULE: &'static str = "catalog";
    const FIELDS: &'static [Field] = &[
        Field::new("shop", Uuid::COLUMNS, false),
        Field::new("name", String::COLUMNS, false),
    ];
    const OWNER: Option<&'static str> = Some("shop");
    const CHANGED: Option<&'static str> = Some("catalog.ProductChanged");
    const PRUNED: Option<&'static str> = Some("catalog.ProductPruned");
    const BEHAVIORS: &'static [&'static dyn Behavior] = &[
        &sofie_db::Timestamps,
        &sofie_db::SoftDelete,
        &sofie_db::Active,
        &sofie_db::Default,
        &Slugged { from: "name" },
        &sofie_db::UniqueCode,
        &sofie_db::Audited,
        &sofie_db::RecordsChanges,
        &sofie_db::PublishLock,
    ];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.shop.encode(out);
        self.name.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            shop: Uuid::decode(row)?,
            name: String::decode(row)?,
        })
    }

    fn owner(&self) -> Option<Uuid> {
        Some(self.shop)
    }

    fn exposed_fields(&self) -> Option<Result<serde_json::Map<String, serde_json::Value>>> {
        let name = serde_json::Value::String(self.name.clone());
        Some(Ok(serde_json::Map::from_iter([("name".to_owned(), name)])))
    }
}

impl Uses<sofie_db::SoftDelete> for Product {}
impl Uses<sofie_db::Active> for Product {}
impl Uses<sofie_db::Default> for Product {}
impl Uses<Slugged> for Product {}
impl Uses<sofie_db::UniqueCode> for Product {}
impl Uses<sofie_db::Audited> for Product {}
impl Uses<sofie_db::RecordsChanges> for Product {}
impl Uses<sofie_db::PublishLock> for Product {}

async fn catalog() -> (TestDatabase, Db) {
    let database = TestDatabase::create().await;
    let postgres = Postgres::connect(database.url()).unwrap();
    postgres
        .transaction(vec![Step::Script(CREATE_PRODUCTS.to_owned())])
        .await
        .unwrap();
    let db = Db::new(postgres);
    db.schema().migrate(&[]).await.unwrap();
    (database, db)
}

async fn added(db: &Db, shop: Uuid, name: &str) -> Product {
    let mut product = Product::new(shop, name);
    db.commit_with(&mut product, Vec::new(), Reach::OwnedBy(shop))
        .await
        .unwrap();
    Product::find(&db.read(), product.id()).await.unwrap()
}

#[tokio::test]
async fn a_new_record_is_active_slugged_coded_and_attributed() {
    let (_database, db) = catalog().await;
    let shop = Uuid::now_v7().unwrap();

    let mut scarf = added(&db, shop, "Blue Wool Scarf").await;
    scarf.name = "Red Scarf".to_owned();
    db.commit(&mut scarf).await.unwrap();

    assert!(scarf.is_active() && !scarf.is_default());
    assert_eq!(scarf.slug(), "red-scarf");
    assert_eq!(scarf.code().len(), 10);
    assert_eq!(scarf.created_by(), Some(shop));
    assert_eq!(scarf.updated_by(), None);
}

async fn renamed(db: &Db, product: &mut Product, name: &str) {
    product.name = name.to_owned();
    db.commit(product).await.unwrap();
}

async fn by_slug(db: &Db, slug: &str) -> Result<String> {
    let found = Product::query().by_slug(&db.read(), slug).await?;
    Ok(found.name)
}

fn moved_to(found: Result<String>) -> String {
    match found {
        Err(Error::Moved { to, .. }) => to,
        other => panic!("expected a move, got {other:?}"),
    }
}

#[tokio::test]
async fn a_previous_slug_moves_to_the_current_one_through_every_rename() {
    let (_database, db) = catalog().await;
    let shop = Uuid::now_v7().unwrap();
    let mut scarf = added(&db, shop, "Blue Scarf").await;

    renamed(&db, &mut scarf, "Red Scarf").await;
    let once = by_slug(&db, "blue-scarf").await;
    renamed(&db, &mut scarf, "Green Scarf").await;

    assert_eq!(moved_to(once), "red-scarf");
    assert_eq!(moved_to(by_slug(&db, "blue-scarf").await), "green-scarf");
    assert_eq!(moved_to(by_slug(&db, "red-scarf").await), "green-scarf");
    assert_eq!(by_slug(&db, "green-scarf").await.unwrap(), "Green Scarf");
    assert!(matches!(by_slug(&db, "hat").await, Err(Error::NotFound)));
}

#[tokio::test]
async fn a_slug_another_record_takes_names_that_record_from_then_on() {
    let (_database, db) = catalog().await;
    let shop = Uuid::now_v7().unwrap();
    let mut scarf = added(&db, shop, "Scarf").await;
    renamed(&db, &mut scarf, "Old Scarf").await;

    let mut hat = added(&db, shop, "Scarf").await;
    let taken = by_slug(&db, "scarf").await.unwrap();
    renamed(&db, &mut hat, "Hat").await;

    assert_eq!(taken, "Scarf");
    assert_eq!(moved_to(by_slug(&db, "scarf").await), "hat");
}

#[tokio::test]
async fn deleting_a_record_drops_its_previous_slugs() {
    let (_database, db) = catalog().await;
    let shop = Uuid::now_v7().unwrap();
    let mut scarf = added(&db, shop, "Scarf").await;
    renamed(&db, &mut scarf, "Red Scarf").await;

    scarf.delete();
    db.commit(&mut scarf).await.unwrap();
    let deleted = by_slug(&db, "red-scarf").await;
    scarf.restore();
    db.commit(&mut scarf).await.unwrap();

    assert!(matches!(deleted, Err(Error::NotFound)));
    assert!(matches!(by_slug(&db, "scarf").await, Err(Error::NotFound)));
    assert_eq!(by_slug(&db, "red-scarf").await.unwrap(), "Red Scarf");
}

#[tokio::test]
async fn active_reads_leave_inactive_records_out() {
    let (_database, db) = catalog().await;
    let shop = Uuid::now_v7().unwrap();
    added(&db, shop, "Scarf").await;
    let mut hat = added(&db, shop, "Hat").await;
    hat.deactivate();
    db.commit(&mut hat).await.unwrap();

    let active = Product::query()
        .scope(active)
        .all(&db.read())
        .await
        .unwrap();

    assert_eq!(
        active
            .iter()
            .map(|product| product.name.as_str())
            .collect::<Vec<_>>(),
        ["Scarf"]
    );
}

#[tokio::test]
async fn one_record_per_owner_is_the_default() {
    let (_database, db) = catalog().await;
    let (shop, other_shop) = (Uuid::now_v7().unwrap(), Uuid::now_v7().unwrap());
    let mut scarf = added(&db, shop, "Scarf").await;
    let mut hat = added(&db, shop, "Hat").await;
    let mut other = added(&db, other_shop, "Other").await;
    scarf.make_default();
    other.make_default();
    db.commit(&mut vec![scarf, other]).await.unwrap();

    hat.make_default();
    db.commit(&mut hat).await.unwrap();

    let mut defaults: Vec<String> = Product::query()
        .where_(<Product as Defaults>::IS_DEFAULT.eq(true))
        .all(&db.read())
        .await
        .unwrap()
        .into_iter()
        .map(|product| product.name)
        .collect();
    defaults.sort();
    assert_eq!(defaults, ["Hat", "Other"]);
}

#[tokio::test]
async fn records_made_the_default_at_once_leave_one_default() {
    let (_database, db) = catalog().await;
    let shop = Uuid::now_v7().unwrap();
    let mut products = Vec::new();
    for index in 0..8 {
        products.push(added(&db, shop, &format!("Product {index}")).await);
    }

    let saves: Vec<_> = products
        .into_iter()
        .map(|mut product| {
            let db = db.clone();
            tokio::spawn(async move {
                product.make_default();
                db.commit(&mut product).await
            })
        })
        .collect();
    for save in saves {
        save.await.unwrap().unwrap();
    }

    let defaults = Product::query()
        .where_(<Product as Defaults>::IS_DEFAULT.eq(true))
        .count(&db.read())
        .await
        .unwrap();
    assert_eq!(defaults, 1);
}

#[tokio::test]
async fn a_published_record_cannot_be_changed() {
    let (_database, db) = catalog().await;
    let mut scarf = added(&db, Uuid::now_v7().unwrap(), "Scarf").await;
    scarf.publish();
    db.commit(&mut scarf).await.unwrap();

    scarf.name = "Hat".to_owned();
    let refused = db.commit(&mut scarf).await;

    assert!(scarf.published_at().is_some());
    assert!(matches!(refused, Err(Error::Rule(_))), "{refused:?}");
}

#[tokio::test]
async fn every_change_is_recorded_and_broadcast_once_with_its_exposed_fields() {
    let (database, db) = catalog().await;
    let shop = Uuid::now_v7().unwrap();
    let mut scarf = added(&db, shop, "Scarf").await;
    scarf.name = "Wool Scarf".to_owned();
    db.commit(&mut scarf).await.unwrap();
    scarf.delete();
    db.commit(&mut scarf).await.unwrap();
    scarf.restore();
    db.commit(&mut scarf).await.unwrap();

    let history = Product::history(&db.read(), scarf.id()).await.unwrap();
    let owned = Read::within(&db, Reach::OwnedBy(Uuid::now_v7().unwrap()));
    let hidden = Product::history(&owned, scarf.id()).await.unwrap();
    let broadcasts = sofie_db::query_values(
        &Postgres::connect(database.url()).unwrap(),
        "SELECT payload->>'name', payload->>'deleted_at' IS NOT NULL FROM sofie.messages \
         WHERE name = 'catalog.ProductChanged' ORDER BY (payload->>'version')::int8",
        &[],
    )
    .await
    .unwrap();

    let kinds: Vec<ChangeKind> = history.iter().map(|change| change.kind).collect();
    assert_eq!(
        kinds,
        [
            ChangeKind::Created,
            ChangeKind::Updated,
            ChangeKind::Deleted,
            ChangeKind::Restored
        ]
    );
    assert_eq!(history[0].fields, ["shop", "name"]);
    assert_eq!(history[0].actor, Some(shop));
    assert_eq!(history[1].fields, ["name"]);
    assert_eq!(hidden, []);
    let text = |name: &str| SqlValue::Text(name.to_owned());
    assert_eq!(
        broadcasts,
        [
            vec![text("Scarf"), SqlValue::Bool(false)],
            vec![text("Wool Scarf"), SqlValue::Bool(false)],
            vec![text("Wool Scarf"), SqlValue::Bool(true)],
            vec![text("Wool Scarf"), SqlValue::Bool(false)],
        ]
    );
}

#[tokio::test]
async fn a_change_made_beyond_the_owner_names_its_actor() {
    let (_database, db) = catalog().await;
    let (shop, staff) = (Uuid::now_v7().unwrap(), Uuid::now_v7().unwrap());
    let mut scarf = added(&db, shop, "Scarf").await;
    scarf.name = "Wool Scarf".to_owned();

    db.commit_with(&mut scarf, Vec::new(), Reach::AllAs(staff))
        .await
        .unwrap();

    let history = Product::history(&db.read(), scarf.id()).await.unwrap();
    let stored = Product::find(&db.read(), scarf.id()).await.unwrap();
    assert_eq!(history[1].actor, Some(staff));
    assert_eq!(stored.created_by(), Some(shop));
    assert_eq!(stored.updated_by(), Some(staff));
}

#[tokio::test]
async fn a_change_names_the_action_it_was_made_for() {
    let (_database, db) = catalog().await;
    let mut scarf = added(&db, Uuid::now_v7().unwrap(), "Scarf").await;
    scarf.name = "Wool Scarf".to_owned();

    sofie_db::action::acting_for("catalog.staff.rename", db.commit(&mut scarf))
        .await
        .unwrap();

    let history = Product::history(&db.read(), scarf.id()).await.unwrap();
    let actions: Vec<Option<&str>> = history
        .iter()
        .map(|change| change.action.as_deref())
        .collect();
    assert_eq!(actions, [None, Some("catalog.staff.rename")]);
}

static HEARD_CHANGES: std::sync::Mutex<Vec<serde_json::Value>> = std::sync::Mutex::new(Vec::new());

struct ProductChanged(serde_json::Value);

impl serde::Serialize for ProductChanged {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl sofie_db::Input for ProductChanged {
    fn from_json(value: serde_json::Value) -> Result<Self> {
        Ok(Self(value))
    }
}

impl sofie_db::Event for ProductChanged {
    const NAME: &'static str = "catalog.ProductChanged";
}

impl sofie_db::Resolve<sofie_db::worker::JobContext> for ProductChanged {
    fn resolve(
        context: &mut sofie_db::worker::JobContext,
    ) -> impl Future<Output = Result<Self>> + Send {
        use sofie_db::{Context, Event, Input};
        std::future::ready(context.event(Self::NAME).and_then(Self::from_json))
    }
}

async fn hear(changed: ProductChanged) -> Result<()> {
    HEARD_CHANGES.lock().unwrap().push(changed.0);
    Ok(())
}

async fn forget_products(prune: sofie_db::Prune, clock: sofie_core::Clock) -> Result<()> {
    let created_at = sofie_db::Column::<Product, sofie_core::Instant>::new("created_at");
    let later = clock
        .now()
        .checked_add(std::time::Duration::from_secs(60))?;
    prune.before(created_at, later).await.map(drop)
}

#[tokio::test]
async fn a_pruned_record_is_dropped_by_a_mirror_that_hides_deleted_ones() {
    let (database, db) = catalog().await;
    Postgres::connect(database.url())
        .unwrap()
        .transaction(vec![Step::Script(
            "CREATE TABLE catalog.product_archive (id uuid PRIMARY KEY, version int8 NOT NULL, \
             deleted_at timestamptz, name text NOT NULL)"
                .to_owned(),
        )])
        .await
        .unwrap();
    sofie_db::provide::<dyn sofie_db::Lookup<()>>(&db, std::sync::Arc::new(NoRecords));
    added(&db, Uuid::now_v7().unwrap(), "Scarf").await;
    let mirror = || sofie_db::worker::Worker::new(db.clone()).mirror::<ProductArchive>("archive");
    mirror().drain().await.unwrap();

    mirror()
        .every("catalog.forget", HOURLY, forget_products)
        .drain()
        .await
        .unwrap();

    let archived = sofie_db::query_values(
        &Postgres::connect(database.url()).unwrap(),
        "SELECT count(*) FROM catalog.product_archive",
        &[],
    )
    .await
    .unwrap();
    assert_eq!(archived, [[SqlValue::Int(0)]]);
}

/// Every message queued from now on, in `public.queued`.
async fn noting_queued(database: &TestDatabase) -> Postgres {
    let raw = Postgres::connect(database.url()).unwrap();
    raw.transaction(vec![Step::Script(
        "CREATE TABLE public.queued (kind text, name text, payload jsonb);
         CREATE FUNCTION public.note_queued() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN INSERT INTO public.queued VALUES (NEW.kind::text, NEW.name, NEW.payload::jsonb);
         RETURN NEW; END $$;
         CREATE TRIGGER noted AFTER INSERT ON sofie.messages
         FOR EACH ROW EXECUTE FUNCTION public.note_queued();"
            .to_owned(),
    )])
    .await
    .unwrap();
    raw
}

#[tokio::test]
async fn a_prune_nobody_listens_to_queues_no_event() {
    let (database, db) = catalog().await;
    added(&db, Uuid::now_v7().unwrap(), "Scarf").await;
    let raw = noting_queued(&database).await;

    sofie_db::worker::Worker::new(db.clone())
        .every("catalog.forget", HOURLY, forget_products)
        .drain()
        .await
        .unwrap();

    let events = sofie_db::query_values(&raw, "SELECT name FROM queued WHERE kind = 'event'", &[])
        .await
        .unwrap();
    assert_eq!(events, Vec::<Vec<SqlValue>>::new());
    let left = Product::query().count(&db.read()).await.unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn a_listener_other_than_a_mirror_is_not_told_of_a_pruned_record() {
    let (_database, db) = catalog().await;
    let scarf = added(&db, Uuid::now_v7().unwrap(), "Scarf").await;
    let worker = sofie_db::worker::Worker::new(db)
        .on::<ProductChanged, _>("catalog.heard", hear)
        .every("catalog.forget", HOURLY, forget_products);

    worker.drain().await.unwrap();

    let heard = HEARD_CHANGES.lock().unwrap().clone();
    let versions: Vec<_> = heard
        .iter()
        .map(|change| change["version"].clone())
        .collect();
    assert_eq!(versions, [1], "{heard:?}");
    assert_eq!(heard[0]["id"], scarf.id().to_string());
}

static JOB_ACTION: std::sync::Mutex<Option<&'static str>> = std::sync::Mutex::new(None);

async fn note_action() -> Result<()> {
    *JOB_ACTION.lock().unwrap() = sofie_db::action::current();
    Ok(())
}

#[tokio::test]
async fn a_job_acts_for_its_registered_name() {
    let (_database, db) = catalog().await;

    sofie_db::worker::Worker::new(db)
        .every("catalog.note_action", HOURLY, note_action)
        .drain()
        .await
        .unwrap();

    assert_eq!(*JOB_ACTION.lock().unwrap(), Some("catalog.note_action"));
}

const HOURLY: std::time::Duration = std::time::Duration::from_secs(3600);

struct NoRecords;

impl sofie_db::Lookup<()> for NoRecords {
    fn get_many<'a>(&'a self, _: &'a [Uuid]) -> sofie_db::BoxFuture<'a, Result<Vec<()>>> {
        Box::pin(std::future::ready(Ok(Vec::new())))
    }

    fn changes(
        &self,
        _: Option<Uuid>,
        _: u64,
    ) -> sofie_db::BoxFuture<'_, Result<Vec<serde_json::Map<String, serde_json::Value>>>> {
        Box::pin(std::future::ready(Ok(Vec::new())))
    }
}

struct ProductArchive {
    meta: Meta,
    name: String,
}

impl Aggregate for ProductArchive {
    const NAME: &'static str = "product_archive";
    const MODULE: &'static str = "catalog";
    const FIELDS: &'static [Field] = &[Field::new("name", String::COLUMNS, false)];
    const BEHAVIORS: &'static [&'static dyn Behavior] = &[&sofie_db::SoftDelete];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.name.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            name: String::decode(row)?,
        })
    }
}

impl sofie_db::Mirror for ProductArchive {
    type Source = ();
    type Changed = ProductChanged;

    fn columns(changed: &serde_json::Map<String, serde_json::Value>) -> Result<Vec<SqlValue>> {
        <ProductCopy as sofie_db::Mirror>::columns(changed)
    }
}

struct ProductCopy {
    meta: Meta,
    name: String,
}

impl Aggregate for ProductCopy {
    const NAME: &'static str = "product_copy";
    const MODULE: &'static str = "catalog";
    const FIELDS: &'static [Field] = &[Field::new("name", String::COLUMNS, false)];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.name.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            name: String::decode(row)?,
        })
    }
}

impl sofie_db::Mirror for ProductCopy {
    type Source = ();
    type Changed = ProductChanged;

    fn columns(changed: &serde_json::Map<String, serde_json::Value>) -> Result<Vec<SqlValue>> {
        let name: String = sofie_db::changed_field(changed, "name")?;
        Ok(vec![SqlValue::Text(name)])
    }
}

const PRODUCT_COPY: &str = "CREATE TABLE catalog.product_copy (id uuid PRIMARY KEY, \
                            version int8 NOT NULL, name text NOT NULL)";

async fn copying(database: &TestDatabase, db: &Db) -> sofie_db::worker::Worker {
    Postgres::connect(database.url())
        .unwrap()
        .transaction(vec![Step::Script(PRODUCT_COPY.to_owned())])
        .await
        .unwrap();
    sofie_db::provide::<dyn sofie_db::Lookup<()>>(db, std::sync::Arc::new(NoRecords));
    sofie_db::worker::Worker::new(db.clone()).mirror::<ProductCopy>("copy")
}

async fn changes_sent(database: &TestDatabase, changes: &[serde_json::Value]) {
    let raw = Postgres::connect(database.url()).unwrap();
    for change in changes {
        sofie_db::query_values(
            &raw,
            "INSERT INTO sofie.messages (id, kind, name, payload, available_at) \
             VALUES (gen_random_uuid(), 'event', 'catalog.ProductChanged', $1, now())",
            &[SqlValue::Json(change.clone())],
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn a_mirror_that_cannot_hide_a_deleted_record_drops_it() {
    let (database, db) = catalog().await;
    let worker = copying(&database, &db).await;
    let id = Uuid::now_v7().unwrap().to_string();
    let deleted_at = sofie_core::Clock::system().now().to_string();
    let changes = [
        serde_json::json!({"id": id, "version": 1, "name": "Scarf"}),
        serde_json::json!({"id": id, "version": 2, "name": "Scarf", "deleted_at": deleted_at}),
    ];

    for change in changes {
        changes_sent(&database, &[change]).await;
        worker.drain().await.unwrap();
    }

    let copies = ProductCopy::query().count(&db.read()).await.unwrap();
    assert_eq!(copies, 0);
}

#[tokio::test]
async fn an_older_change_arriving_after_a_dropped_record_does_not_bring_it_back() {
    let (database, db) = catalog().await;
    let worker = copying(&database, &db).await;
    let id = Uuid::now_v7().unwrap().to_string();
    let deleted_at = sofie_core::Clock::system().now().to_string();
    let changes = [
        serde_json::json!({"id": id, "version": 3, "name": "Scarf", "deleted_at": deleted_at}),
        serde_json::json!({"id": id, "version": 2, "name": "Scarf"}),
    ];

    for change in changes {
        changes_sent(&database, &[change]).await;
        worker.drain().await.unwrap();
    }

    let copies = ProductCopy::query().count(&db.read()).await.unwrap();
    assert_eq!(copies, 0);
}

#[tokio::test]
async fn a_create_and_a_delete_mirrored_at_once_leave_no_row() {
    let (database, db) = catalog().await;
    let worker = copying(&database, &db).await;
    let deleted_at = sofie_core::Clock::system().now().to_string();
    let mut changes = Vec::new();
    for _ in 0..40 {
        let id = Uuid::now_v7().unwrap().to_string();
        changes.push(serde_json::json!({"id": id, "version": 1, "name": "Scarf"}));
        changes.push(
            serde_json::json!({"id": id, "version": 2, "name": "Scarf", "deleted_at": deleted_at}),
        );
    }
    changes_sent(&database, &changes).await;

    worker
        .batch(std::num::NonZeroUsize::new(80).unwrap())
        .drain()
        .await
        .unwrap();

    let copies = ProductCopy::query().count(&db.read()).await.unwrap();
    assert_eq!(copies, 0);
}

#[tokio::test]
async fn a_change_with_a_field_named_pruned_is_still_mirrored() {
    let (database, db) = catalog().await;
    let worker = copying(&database, &db).await;
    let id = Uuid::now_v7().unwrap().to_string();
    let change = serde_json::json!({"id": id, "version": 1, "name": "Scarf", "pruned": true});

    changes_sent(&database, &[change]).await;
    worker.drain().await.unwrap();

    let copies = ProductCopy::query().count(&db.read()).await.unwrap();
    assert_eq!(copies, 1);
}

async fn ignore(_: ProductChanged) -> Result<()> {
    Ok(())
}

#[tokio::test]
async fn a_pruned_change_is_handed_only_to_the_listeners_taking_it() {
    let (database, db) = catalog().await;
    Postgres::connect(database.url())
        .unwrap()
        .transaction(vec![Step::Script(
            "CREATE TABLE catalog.product_archive (id uuid PRIMARY KEY, version int8 NOT NULL, \
             deleted_at timestamptz, name text NOT NULL)"
                .to_owned(),
        )])
        .await
        .unwrap();
    let worker = copying(&database, &db)
        .await
        .mirror::<ProductArchive>("archive")
        .on::<ProductChanged, _>("catalog.ignored", ignore)
        .every("catalog.forget", HOURLY, forget_products);
    added(&db, Uuid::now_v7().unwrap(), "Scarf").await;
    let raw = noting_queued(&database).await;

    worker.drain().await.unwrap();

    let handed = sofie_db::query_values(
        &raw,
        "SELECT name FROM queued WHERE kind = 'job' AND payload ? 'sofie.pruned' ORDER BY name",
        &[],
    )
    .await
    .unwrap();
    let text = |name: &str| vec![SqlValue::Text(name.to_owned())];
    assert_eq!(handed, [text("archive"), text("copy")]);
}

struct Listed(Vec<serde_json::Map<String, serde_json::Value>>);

impl sofie_db::Lookup<()> for Listed {
    fn get_many<'a>(&'a self, _: &'a [Uuid]) -> sofie_db::BoxFuture<'a, Result<Vec<()>>> {
        Box::pin(std::future::ready(Ok(Vec::new())))
    }

    fn changes(
        &self,
        after: Option<Uuid>,
        _: u64,
    ) -> sofie_db::BoxFuture<'_, Result<Vec<serde_json::Map<String, serde_json::Value>>>> {
        let page = if after.is_none() {
            self.0.clone()
        } else {
            Vec::new()
        };
        Box::pin(std::future::ready(Ok(page)))
    }
}

#[tokio::test]
async fn a_backfill_takes_one_lock_and_a_change_two() {
    let (database, db) = catalog().await;
    let worker = copying(&database, &db).await;
    let raw = Postgres::connect(database.url()).unwrap();
    raw.transaction(vec![Step::Script(
        "CREATE TABLE public.locks_held (held int8);
         CREATE FUNCTION public.note_locks() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN INSERT INTO public.locks_held SELECT count(*) FROM pg_locks
         WHERE locktype = 'advisory' AND pid = pg_backend_pid(); RETURN NEW; END $$;
         CREATE TRIGGER noted AFTER INSERT ON catalog.product_copy
         FOR EACH ROW EXECUTE FUNCTION public.note_locks();"
            .to_owned(),
    )])
    .await
    .unwrap();
    let change = |name: &str| {
        let id = Uuid::now_v7().unwrap().to_string();
        serde_json::json!({"id": id, "version": 1, "name": name})
    };
    let listed = ["Scarf", "Hat", "Glove"].map(|name| change(name).as_object().unwrap().clone());
    sofie_db::provide::<dyn sofie_db::Lookup<()>>(&db, std::sync::Arc::new(Listed(listed.into())));

    worker.drain().await.unwrap();
    changes_sent(&database, &[change("Coat")]).await;
    worker.drain().await.unwrap();

    let held = sofie_db::query_values(&raw, "SELECT held FROM public.locks_held", &[])
        .await
        .unwrap();
    let held: Vec<SqlValue> = held.into_iter().flatten().collect();
    assert_eq!(held, [1, 1, 1, 2].map(SqlValue::Int));
}

#[tokio::test]
async fn a_mirror_s_worker_sweeps_records_forgotten_a_month_ago() {
    let (database, db) = catalog().await;
    let raw = Postgres::connect(database.url()).unwrap();
    let (old, recent) = (Uuid::now_v7().unwrap(), Uuid::now_v7().unwrap());
    raw.transaction(vec![Step::Script(format!(
        "INSERT INTO sofie.mirror_forgotten (mirror, id, version, forgotten_at) VALUES \
         ('catalog.product_copy', '{old}', 2, now() - interval '31 days'), \
         ('catalog.product_copy', '{recent}', 2, now() - interval '29 days')"
    ))])
    .await
    .unwrap();
    sofie_db::provide::<dyn sofie_db::Lookup<()>>(&db, std::sync::Arc::new(NoRecords));

    sofie_db::worker::Worker::new(db)
        .mirror::<ProductCopy>("copy")
        .drain()
        .await
        .unwrap();

    let kept = sofie_db::query_values(&raw, "SELECT id FROM sofie.mirror_forgotten", &[])
        .await
        .unwrap();
    assert_eq!(kept, [[SqlValue::Uuid(recent)]]);
}

#[tokio::test]
async fn a_mirror_s_worker_keeps_forgotten_records_while_a_late_change_may_still_be_retried() {
    let (database, db) = catalog().await;
    let raw = Postgres::connect(database.url()).unwrap();
    let (older, newer) = (Uuid::now_v7().unwrap(), Uuid::now_v7().unwrap());
    raw.transaction(vec![Step::Script(format!(
        "INSERT INTO sofie.mirror_forgotten (mirror, id, version, forgotten_at) VALUES \
         ('catalog.product_copy', '{older}', 2, now() - interval '50 days'), \
         ('catalog.product_copy', '{newer}', 2, now() - interval '31 days')"
    ))])
    .await
    .unwrap();

    copying(&database, &db)
        .await
        .max_attempts(std::num::NonZeroU16::new(1000).unwrap())
        .drain()
        .await
        .unwrap();

    let kept = sofie_db::query_values(&raw, "SELECT id FROM sofie.mirror_forgotten", &[])
        .await
        .unwrap();
    assert_eq!(kept, [[SqlValue::Uuid(newer)]]);
}

struct Banner {
    meta: Meta,
}

impl Aggregate for Banner {
    const NAME: &'static str = "banner";
    const MODULE: &'static str = "catalog";
    const FIELDS: &'static [Field] = &[];
    const CHANGED: Option<&'static str> = Some("catalog.BannerChanged");
    const BEHAVIORS: &'static [&'static dyn Behavior] =
        &[&sofie_db::Default, &sofie_db::Broadcasts];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, _: &mut Vec<SqlValue>) {}

    fn decode(meta: Meta, _: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self { meta })
    }
}

impl Uses<sofie_db::Default> for Banner {}

#[test]
#[should_panic(expected = "`banner` uses both `Default` and `Broadcasts`")]
fn a_broadcasting_aggregate_cannot_take_defaults() {
    let _ = <Banner as sofie_db::Source>::columns();
}

#[derive(Default)]
struct SeenChanges(std::sync::Mutex<Vec<sofie_db::ChangedRecord>>);

impl sofie_db::CommitHook for SeenChanges {
    fn committed(&self, changes: &[sofie_db::ChangedRecord]) {
        self.0.lock().unwrap().extend_from_slice(changes);
    }
}

#[tokio::test]
async fn a_kept_mirror_row_is_told_to_the_commit_hooks() {
    let (database, db) = catalog().await;
    let worker = copying(&database, &db).await;
    let seen = std::sync::Arc::new(SeenChanges::default());
    sofie_db::add_commit_hook(&db, seen.clone());
    let id = Uuid::now_v7().unwrap();
    let change = serde_json::json!({"id": id.to_string(), "version": 1, "name": "Scarf"});

    changes_sent(&database, &[change]).await;
    worker.drain().await.unwrap();

    let seen = seen.0.lock().unwrap().clone();
    let copy = sofie_db::ChangedRecord {
        module: "catalog",
        aggregate: "product_copy",
        id: Some(id),
    };
    assert_eq!(seen, [copy]);
}

#[tokio::test]
async fn making_a_default_tells_the_commit_hooks_any_record_may_have_changed() {
    let (_database, db) = catalog().await;
    let shop = Uuid::now_v7().unwrap();
    let mut scarf = added(&db, shop, "Scarf").await;
    scarf.make_default();
    db.commit(&mut scarf).await.unwrap();
    let mut hat = added(&db, shop, "Hat").await;
    let seen = std::sync::Arc::new(SeenChanges::default());
    sofie_db::add_commit_hook(&db, seen.clone());

    hat.make_default();
    db.commit(&mut hat).await.unwrap();

    let seen = seen.0.lock().unwrap().clone();
    let any_product = sofie_db::ChangedRecord {
        module: "catalog",
        aggregate: "product",
        id: None,
    };
    assert!(seen.contains(&any_product), "{seen:?}");
}

#[tokio::test]
async fn a_handed_over_record_s_history_is_read_by_its_owner_now() {
    let (_database, db) = catalog().await;
    let (old_shop, new_shop) = (Uuid::now_v7().unwrap(), Uuid::now_v7().unwrap());
    let mut scarf = added(&db, old_shop, "Scarf").await;
    scarf.shop = new_shop;
    db.commit(&mut scarf).await.unwrap();

    let old_owner = Read::within(&db, Reach::OwnedBy(old_shop));
    let new_owner = Read::within(&db, Reach::OwnedBy(new_shop));
    let old_owner = Product::history(&old_owner, scarf.id()).await.unwrap();
    let new_owner = Product::history(&new_owner, scarf.id()).await.unwrap();

    assert_eq!(old_owner, []);
    let kinds: Vec<ChangeKind> = new_owner.iter().map(|change| change.kind).collect();
    assert_eq!(kinds, [ChangeKind::Created, ChangeKind::Updated]);
}

async fn mark_seen(changed: ProductChanged, read: Read) -> Result<Option<Product>> {
    if changed.0["name"] != "Wool Scarf" {
        return Ok(None);
    }
    let id = changed.0["id"].as_str().unwrap().parse().unwrap();
    let mut product = Product::find(&read, sofie_db::Id::from_uuid(id)).await?;
    "Wool Scarf (seen)".clone_into(&mut product.name);
    Ok(Some(product))
}

/// Renames a product as `staff`, whose change a listener then marks seen.
async fn marked_after_a_staff_rename(heard_first: bool) -> (Uuid, Vec<sofie_db::Change>, Product) {
    let (_database, db) = catalog().await;
    let staff = Uuid::now_v7().unwrap();
    let worker = sofie_db::worker::Worker::new(db.clone())
        .on::<ProductChanged, _>("catalog.mark_seen", mark_seen)
        .on::<ProductChanged, _>("catalog.ignore", ignore);
    if heard_first {
        sofie_db::worker::publish_heard(&worker);
    }
    let mut scarf = added(&db, Uuid::now_v7().unwrap(), "Scarf").await;
    "Wool Scarf".clone_into(&mut scarf.name);
    db.commit_with(&mut scarf, Vec::new(), Reach::AllAs(staff))
        .await
        .unwrap();

    worker.drain().await.unwrap();

    let history = Product::history(&db.read(), scarf.id()).await.unwrap();
    let stored = Product::find(&db.read(), scarf.id()).await.unwrap();
    (staff, history, stored)
}

#[tokio::test]
async fn a_listener_of_a_member_s_change_writes_as_that_member() {
    for heard_first in [true, false] {
        let (staff, history, stored) = marked_after_a_staff_rename(heard_first).await;

        assert_eq!(
            stored.name, "Wool Scarf (seen)",
            "heard first: {heard_first}"
        );
        assert_eq!(stored.updated_by(), Some(staff));
        let marked = history.last().unwrap();
        assert_eq!(marked.actor, Some(staff), "heard first: {heard_first}");
        assert_eq!(marked.action.as_deref(), Some("catalog.mark_seen"));
    }
}

async fn rename_every_product(read: Read) -> Result<Vec<Product>> {
    let mut products = Product::query().all(&read).await?;
    for product in &mut products {
        "Renamed".clone_into(&mut product.name);
    }
    Ok(products)
}

#[tokio::test]
async fn a_scheduled_job_writes_as_nobody() {
    let (_database, db) = catalog().await;
    let scarf = added(&db, Uuid::now_v7().unwrap(), "Scarf").await;

    sofie_db::worker::Worker::new(db.clone())
        .every("catalog.rename", HOURLY, rename_every_product)
        .drain()
        .await
        .unwrap();

    let history = Product::history(&db.read(), scarf.id()).await.unwrap();
    let renamed = history.last().unwrap();
    assert_eq!(renamed.action.as_deref(), Some("catalog.rename"));
    assert_eq!(renamed.actor, None);
}
