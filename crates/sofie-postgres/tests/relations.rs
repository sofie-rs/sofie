use sofie_core::{Cast, ColumnReader, Error, Result, SqlType, SqlValue, Uuid};
use sofie_db::{
    Aggregate, Child, ChildRows, ChildTable, Children, Column, Condition, Db, Driver, Field, Id,
    Load, Meta, Reach, Read, Select, Selected, Source, Step, joined, owned_through, read_columns,
    refuse_parent_beyond_reach,
};
use sofie_postgres::Postgres;
use sofie_testing::TestDatabase;

const CREATE_TABLES: &str = "CREATE SCHEMA crm;
CREATE TABLE crm.member (
    id uuid PRIMARY KEY,
    version int8 NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    account uuid NOT NULL,
    name text NOT NULL
);
CREATE TABLE crm.basket (
    id uuid PRIMARY KEY,
    version int8 NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    member uuid NOT NULL REFERENCES crm.member (id),
    label text NOT NULL
);
CREATE TABLE crm.basket_item (
    id uuid PRIMARY KEY,
    basket uuid NOT NULL REFERENCES crm.basket (id),
    product text NOT NULL,
    quantity int8 NOT NULL
)";

struct Member {
    meta: Meta,
    account: Uuid,
    name: String,
}

impl Aggregate for Member {
    const NAME: &'static str = "member";
    const MODULE: &'static str = "crm";
    const FIELDS: &'static [Field] = &[
        Field::new("account", Uuid::COLUMNS, false),
        Field::new("name", String::COLUMNS, false),
    ];
    const OWNER: Option<&'static str> = Some("account");
    const BEHAVIORS: &'static [&'static dyn sofie_db::Behavior] = &[&sofie_db::Timestamps];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.account.encode(out);
        self.name.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            account: Uuid::decode(row)?,
            name: String::decode(row)?,
        })
    }

    fn owner(&self) -> Option<Uuid> {
        Some(self.account)
    }
}

impl Member {
    const NAME_COLUMN: Column<Self, String> = Column::new("name");
}

struct BasketItem {
    id: Id<BasketItem>,
    product: String,
    quantity: i64,
}

impl BasketItem {
    fn new(product: &str, quantity: i64) -> Self {
        Self {
            id: Id::from_uuid(Uuid::now_v7().unwrap()),
            product: product.to_owned(),
            quantity,
        }
    }
}

impl Child for BasketItem {
    const TABLE: ChildTable = ChildTable {
        module: "crm",
        name: "basket_item",
        fields: &[
            Field::new("product", String::COLUMNS, false),
            Field::new("quantity", i64::COLUMNS, false),
        ],
    };

    fn id(&self) -> Uuid {
        self.id.uuid()
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.product.encode(out);
        self.quantity.encode(out);
    }

    fn decode(id: Uuid, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            id: Id::from_uuid(id),
            product: String::decode(row)?,
            quantity: i64::decode(row)?,
        })
    }
}

struct Basket {
    meta: Meta,
    member: Id<Member>,
    label: String,
    items: Children<BasketItem>,
}

impl Basket {
    const MEMBER: Column<Self, Id<Member>> = Column::new("member");

    fn new(member: Id<Member>, label: &str) -> Self {
        Self {
            meta: Meta::new().unwrap(),
            member,
            label: label.to_owned(),
            items: Children::default(),
        }
    }
}

impl Aggregate for Basket {
    const NAME: &'static str = "basket";
    const MODULE: &'static str = "crm";
    const FIELDS: &'static [Field] = &[
        Field::new("member", Uuid::COLUMNS, false),
        Field::new("label", String::COLUMNS, false),
    ];
    const OWNER: Option<&'static str> = Some("member");
    const BEHAVIORS: &'static [&'static dyn sofie_db::Behavior] = &[&sofie_db::Timestamps];
    const CHILDREN: &'static [&'static ChildTable] = &[&BasketItem::TABLE];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.member.encode(out);
        self.label.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            member: Id::decode(row)?,
            label: String::decode(row)?,
            items: Children::decode(row)?,
        })
    }

    fn owned_by(owner: Uuid) -> Option<Condition<Self>> {
        owned_through(Self::MEMBER, owner)
    }

    fn children(&self) -> Vec<&dyn ChildRows> {
        vec![&self.items]
    }

    fn children_mut(&mut self) -> Vec<&mut dyn ChildRows> {
        vec![&mut self.items]
    }
}

struct BasketRow {
    label: String,
    member_name: Option<String>,
}

impl Source for BasketRow {
    type Base = Basket;

    fn columns() -> Vec<Selected> {
        [
            read_columns::<Basket>(&["label"]),
            joined(Basket::MEMBER, Member::NAME_COLUMN),
        ]
        .concat()
    }

    fn column_types() -> Vec<Option<SqlType>> {
        sofie_db::column_types(&[String::COLUMNS, String::COLUMNS])
    }

    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            label: String::decode(row)?,
            member_name: Option::decode(row)?,
        })
    }
}

async fn crm() -> (TestDatabase, Db) {
    let database = TestDatabase::create().await;
    let postgres = Postgres::connect(database.url()).unwrap();
    postgres
        .transaction(vec![Step::Script(CREATE_TABLES.to_owned())])
        .await
        .unwrap();
    (database, Db::new(postgres))
}

async fn member(db: &Db, account: Uuid, name: &str) -> Id<Member> {
    let mut member = Member {
        meta: Meta::new().unwrap(),
        account,
        name: name.to_owned(),
    };
    db.commit(&mut member).await.unwrap();
    member.id()
}

fn products(basket: &Basket) -> Vec<(&str, i64)> {
    basket
        .items
        .iter()
        .map(|item| (item.product.as_str(), item.quantity))
        .collect()
}

#[tokio::test]
async fn an_aggregate_loads_and_saves_its_child_rows_by_id() {
    let (_database, db) = crm().await;
    let ada = member(&db, Uuid::now_v7().unwrap(), "Ada").await;
    let mut basket = Basket::new(ada, "weekly");
    basket.items.push(BasketItem::new("tea", 2));
    basket.items.push(BasketItem::new("milk", 1));
    db.commit(&mut basket).await.unwrap();

    let mut loaded = Basket::find(&db.read(), basket.id()).await.unwrap();
    assert_eq!(products(&loaded), [("tea", 2), ("milk", 1)]);
    loaded.items[0].quantity = 3;
    loaded.items.retain(|item| item.product != "milk");
    loaded.items.push(BasketItem::new("bread", 1));
    db.commit(&mut loaded).await.unwrap();
    db.commit(&mut loaded).await.unwrap();

    let reloaded = Basket::find(&db.read(), basket.id()).await.unwrap();
    assert_eq!(products(&reloaded), [("tea", 3), ("bread", 1)]);
    assert_eq!(reloaded.meta().version(), 2);
    let empty = Basket::find(&db.read(), Basket::new(ada, "none").id()).await;
    assert!(matches!(empty, Err(Error::NotFound)));
}

#[tokio::test]
async fn a_stale_change_to_child_rows_is_a_conflict() {
    let (_database, db) = crm().await;
    let ada = member(&db, Uuid::now_v7().unwrap(), "Ada").await;
    let mut basket = Basket::new(ada, "weekly");
    basket.items.push(BasketItem::new("tea", 2));
    db.commit(&mut basket).await.unwrap();
    let mut first = Basket::find(&db.read(), basket.id()).await.unwrap();
    let mut second = Basket::find(&db.read(), basket.id()).await.unwrap();

    first.items[0].quantity = 5;
    db.commit(&mut first).await.unwrap();
    second.items[0].quantity = 9;

    assert!(matches!(db.commit(&mut second).await, Err(Error::Conflict)));
    let kept = Basket::find(&db.read(), basket.id()).await.unwrap();
    assert_eq!(products(&kept), [("tea", 5)]);
}

#[tokio::test]
async fn a_record_owned_through_its_parent_is_kept_to_the_parents_owner() {
    let (_database, db) = crm().await;
    let (ada_account, bob_account) = (Uuid::now_v7().unwrap(), Uuid::now_v7().unwrap());
    let ada = member(&db, ada_account, "Ada").await;
    let bob = member(&db, bob_account, "Bob").await;
    db.commit(&mut vec![
        Basket::new(ada, "ada's"),
        Basket::new(bob, "bob's"),
    ])
    .await
    .unwrap();
    let as_ada = Read::within(&db, Reach::OwnedBy(ada_account));

    let seen = Basket::query().all(&as_ada).await.unwrap();
    let mut forged = Basket::new(bob, "forged");
    let refused = db
        .commit_with(&mut forged, Vec::new(), Reach::OwnedBy(ada_account))
        .await;
    let mut own = Basket::new(ada, "own");
    let allowed = db
        .commit_with(&mut own, Vec::new(), Reach::OwnedBy(ada_account))
        .await;

    assert_eq!(
        seen.iter()
            .map(|basket| basket.label.as_str())
            .collect::<Vec<_>>(),
        ["ada's"]
    );
    assert!(matches!(refused, Err(Error::NotFound)), "{refused:?}");
    assert!(allowed.is_ok(), "{allowed:?}");
    assert_eq!(Basket::query().count(&db.read()).await.unwrap(), 3);
}

#[tokio::test]
async fn an_owned_area_cannot_take_over_another_principal_s_record() {
    let (_database, db) = crm().await;
    let (ada_account, bob_account) = (Uuid::now_v7().unwrap(), Uuid::now_v7().unwrap());
    let ada = member(&db, ada_account, "Ada").await;
    let bob = member(&db, bob_account, "Bob").await;
    let mut bob_s = Basket::new(bob, "bob's");
    db.commit(&mut bob_s).await.unwrap();
    let as_ada = Reach::OwnedBy(ada_account);

    let mut member = Member::find(&db.read(), bob).await.unwrap();
    member.account = ada_account;
    let member_taken = db.commit_with(&mut member, Vec::new(), as_ada).await;
    let mut basket = Basket::find(&db.read(), bob_s.id()).await.unwrap();
    basket.member = ada;
    let basket_taken = db.commit_with(&mut basket, Vec::new(), as_ada).await;

    assert!(member_taken.is_err(), "{member_taken:?}");
    assert!(
        matches!(basket_taken, Err(Error::NotFound)),
        "{basket_taken:?}"
    );
    let kept = Basket::find(&db.read(), bob_s.id()).await.unwrap();
    assert_eq!(kept.member, bob);
}

#[tokio::test]
async fn a_read_model_joins_a_column_of_the_record_its_base_names() {
    let (_database, db) = crm().await;
    let account = Uuid::now_v7().unwrap();
    let ada = member(&db, account, "Ada").await;
    db.commit(&mut Basket::new(ada, "weekly")).await.unwrap();

    let rows = BasketRow::query()
        .where_(Basket::MEMBER.eq(ada))
        .all(&db.read())
        .await
        .unwrap();
    let owned = BasketRow::query()
        .all(&Read::within(&db, Reach::OwnedBy(account)))
        .await
        .unwrap();

    assert_eq!(rows[0].label, "weekly");
    assert_eq!(rows[0].member_name.as_deref(), Some("Ada"));
    assert_eq!(owned[0].member_name.as_deref(), Some("Ada"));
}

#[tokio::test]
async fn the_schema_check_holds_child_tables_to_their_fields() {
    use sofie_db::{AggregateTable, Migrations, Mismatch, MismatchKind};

    const CRM: Migrations = Migrations::new("crm", &[]).aggregates(&[
        AggregateTable::of::<Member>(),
        AggregateTable::of::<Basket>(),
    ]);
    let (database, db) = crm().await;
    assert_eq!(db.schema().check(&[CRM]).await.unwrap(), []);

    Postgres::connect(database.url())
        .unwrap()
        .transaction(vec![Step::Script(
            "ALTER TABLE crm.basket_item DROP COLUMN quantity, \
             ALTER COLUMN product TYPE int8 USING 0"
                .to_owned(),
        )])
        .await
        .unwrap();

    let item = |kind| Mismatch {
        table: "crm.basket_item".to_owned(),
        kind,
    };
    assert_eq!(
        db.schema().check(&[CRM]).await.unwrap(),
        [
            item(MismatchKind::Type {
                column: "product".to_owned(),
                expected: SqlType::Text,
                found: "int8".to_owned(),
            }),
            item(MismatchKind::MissingColumn {
                column: "quantity".to_owned(),
                expected: SqlType::Int,
            }),
        ]
    );
}

#[tokio::test]
async fn a_list_under_another_principal_s_parent_is_not_found() {
    let (_database, db) = crm().await;
    let (ada_account, bob_account) = (Uuid::now_v7().unwrap(), Uuid::now_v7().unwrap());
    let ada = member(&db, ada_account, "Ada").await;
    let bob = member(&db, bob_account, "Bob").await;
    let as_ada = Read::within(&db, Reach::OwnedBy(ada_account));

    let own = refuse_parent_beyond_reach::<Basket>(&as_ada, "member", ada.uuid()).await;
    let other = refuse_parent_beyond_reach::<Basket>(&as_ada, "member", bob.uuid()).await;

    assert!(own.is_ok(), "{own:?}");
    assert!(matches!(other, Err(Error::NotFound)), "{other:?}");
}
