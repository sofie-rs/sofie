use sofie_core::{Cast, ColumnReader, Error, FieldError, Result, SqlValue, Uuid};
use sofie_db::{
    Across, Aggregate, Behavior, Column, Db, Field, Guard, Load, Meta, Migrations, Reach, Read,
    Select, SoftDeletes, Uses,
};
use sofie_sqlite::Sqlite;

const BOOKINGS: &str = "CREATE TABLE scheduling_booking (
    id uuid PRIMARY KEY,
    version bigint NOT NULL,
    deleted_at timestamptz,
    account uuid NOT NULL,
    slot bigint NOT NULL
)";

const MIGRATIONS: Migrations = Migrations::new(
    "scheduling",
    &[sofie_db::File::new(
        "0001_bookings.sql",
        BOOKINGS.as_bytes(),
    )],
);

struct Booking {
    meta: Meta,
    account: Uuid,
    slot: i64,
}

impl Booking {
    const SLOT: Column<Self, i64> = Column::new("slot");

    fn new(account: Uuid, slot: i64) -> Self {
        Self {
            meta: Meta::new().unwrap(),
            account,
            slot,
        }
    }
}

impl Aggregate for Booking {
    const NAME: &'static str = "booking";
    const MODULE: &'static str = "scheduling";
    const FIELDS: &'static [Field] = &[
        Field::new("account", Uuid::COLUMNS, false),
        Field::new("slot", i64::COLUMNS, false),
    ];
    const OWNER: Option<&'static str> = Some("account");
    const BEHAVIORS: &'static [&'static dyn Behavior] = &[&sofie_db::SoftDelete];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.account.encode(out);
        self.slot.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            account: Uuid::decode(row)?,
            slot: i64::decode(row)?,
        })
    }

    fn owner(&self) -> Option<Uuid> {
        Some(self.account)
    }
}

impl Uses<sofie_db::SoftDelete> for Booking {}

/// A file, so each `Db` has its own connection and two commits can truly race.
struct File(std::path::PathBuf);

impl File {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("sofie-across-{name}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    fn db(&self) -> Db {
        Db::new(Sqlite::open(&self.0).unwrap())
    }
}

impl Drop for File {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}

async fn scheduling() -> Db {
    let db = Db::new(Sqlite::memory().unwrap());
    db.schema().migrate(&[MIGRATIONS]).await.unwrap();
    db
}

fn across(read: &Read, slot: i64) -> Across<Booking> {
    Across::for_derived_across_only(read).where_(Booking::SLOT.eq(slot))
}

fn taken() -> FieldError {
    FieldError::invalid("slot", "is already booked")
}

fn slot_guard(db: &Db, owner: Uuid, slot: i64) -> Guard<Booking> {
    across(&Read::within(db, Reach::OwnedBy(owner)), slot).guard(taken())
}

async fn book(db: &Db, owner: Uuid, slot: i64) -> Result<()> {
    let mut booking = (Booking::new(owner, slot), slot_guard(db, owner, slot));
    db.commit_with(&mut booking, Vec::new(), Reach::OwnedBy(owner))
        .await
}

async fn booked(db: &Db, owner: Uuid, slot: i64) -> Booking {
    let mut booking = Booking::new(owner, slot);
    db.commit_with(&mut booking, Vec::new(), Reach::OwnedBy(owner))
        .await
        .unwrap();
    booking
}

fn refused_as_taken(outcome: &Result<()>) -> bool {
    matches!(outcome, Err(Error::Invalid(fields)) if fields[..] == [taken()])
}

#[tokio::test]
async fn an_owned_area_checks_every_owner_s_records_but_lists_only_its_own() {
    let db = scheduling().await;
    let (ada, bob) = (Uuid::now_v7().unwrap(), Uuid::now_v7().unwrap());
    booked(&db, bob, 10).await;
    booked(&db, bob, 10).await;
    let mut cancelled = booked(&db, bob, 11).await;
    cancelled.delete();
    db.commit(&mut cancelled).await.unwrap();
    let as_ada = Read::within(&db, Reach::OwnedBy(ada));

    let listed = Booking::query()
        .where_(Booking::SLOT.eq(10))
        .exists(&as_ada)
        .await
        .unwrap();

    assert!(!listed);
    assert!(across(&as_ada, 10).exists().await.unwrap());
    assert_eq!(across(&as_ada, 10).count().await.unwrap(), 2);
    assert!(!across(&as_ada, 11).exists().await.unwrap());
    assert_eq!(across(&as_ada, 11).count().await.unwrap(), 0);
}

#[tokio::test]
async fn a_guard_refuses_a_commit_whose_slot_another_owner_holds() {
    let db = scheduling().await;
    let (ada, bob) = (Uuid::now_v7().unwrap(), Uuid::now_v7().unwrap());
    booked(&db, bob, 10).await;

    let refused = book(&db, ada, 10).await;
    let free = book(&db, ada, 12).await;

    assert!(refused_as_taken(&refused), "{refused:?}");
    assert!(free.is_ok(), "{free:?}");
    let all = Read::within(&db, Reach::All);
    assert_eq!(across(&all, 10).count().await.unwrap(), 1);
}

#[tokio::test]
async fn an_update_passes_its_own_guard_when_the_guard_excepts_it() {
    let db = scheduling().await;
    let ada = Uuid::now_v7().unwrap();
    let id = booked(&db, ada, 10).await.id();
    let as_ada = Read::within(&db, Reach::OwnedBy(ada));
    let kept = |guard| async {
        let mut update = (Booking::find(&as_ada, id).await.unwrap(), guard);
        db.commit_with(&mut update, Vec::new(), Reach::OwnedBy(ada))
            .await
    };

    let matched_itself = kept(slot_guard(&db, ada, 10)).await;
    let passed = kept(slot_guard(&db, ada, 10).except(id)).await;

    assert!(refused_as_taken(&matched_itself), "{matched_itself:?}");
    assert!(passed.is_ok(), "{passed:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_owners_racing_for_one_slot_on_two_connections_get_one_booking_and_one_refusal() {
    let file = File::new("race");
    let first_db = file.db();
    first_db.schema().migrate(&[MIGRATIONS]).await.unwrap();
    let second_db = file.db();
    let (ada, bob) = (Uuid::now_v7().unwrap(), Uuid::now_v7().unwrap());

    for slot in 0..20 {
        let racing = |db: &Db, owner| {
            let db = db.clone();
            tokio::spawn(async move { book(&db, owner, slot).await })
        };
        let (first, second) = (racing(&first_db, ada), racing(&second_db, bob));
        let (first, second) = (first.await.unwrap(), second.await.unwrap());

        let outcomes = [&first, &second];
        let booked = outcomes.iter().filter(|outcome| outcome.is_ok()).count();
        let refused = outcomes
            .iter()
            .filter(|outcome| refused_as_taken(outcome))
            .count();
        assert_eq!(
            (booked, refused),
            (1, 1),
            "slot {slot}: {first:?} / {second:?}"
        );
    }
}
