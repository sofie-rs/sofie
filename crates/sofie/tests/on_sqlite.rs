#![cfg(feature = "sqlite")]

use sofie::core::Instant;
use sofie::db::{Aggregate as _, Db, File, Meta, Migrations, Select};
use sofie::sqlite::Sqlite;
use sofie::values::{Currency, Money};
use sofie::{Aggregate, Enum, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
enum Status {
    #[to(Paid)]
    Pending,
    Paid,
}

#[derive(Debug, Clone, PartialEq, Value)]
struct Size {
    width: i64,
    label: String,
}

impl Size {
    #[expect(
        clippy::unnecessary_wraps,
        reason = "derive(Value) builds a value through a fallible try_new"
    )]
    fn try_new(width: i64, label: String) -> Result<Self, &'static str> {
        Ok(Self { width, label })
    }
}

#[derive(Aggregate)]
struct Parcel {
    meta: Meta,
    status: Status,
    total: Money,
    discount: Option<Money>,
    note: Option<String>,
    size: Size,
    ratio: f64,
    placed_at: Instant,
}

const PARCELS: Migrations = Migrations::new("on_sqlite", &[File::new("0001_parcels.sql", b"")])
    .sqlite(&[File::new(
        "0001_parcels.sql",
        b"CREATE TABLE on_sqlite_parcel (
        id uuid PRIMARY KEY,
        version bigint NOT NULL,
        created_at timestamptz NOT NULL,
        updated_at timestamptz NOT NULL,
        status text NOT NULL,
        total bigint NOT NULL,
        total_currency text NOT NULL,
        discount bigint,
        discount_currency text,
        note text,
        size_width bigint NOT NULL,
        size_label text NOT NULL,
        ratio double precision NOT NULL,
        placed_at timestamptz NOT NULL
    )",
    )]);

fn euros(minor: i64) -> Money {
    Money::new(minor, Currency::parse("EUR").unwrap())
}

fn parcel(total: i64, note: Option<&str>) -> Parcel {
    Parcel {
        meta: Meta::new().unwrap(),
        status: Status::Pending,
        total: euros(total),
        discount: None,
        note: note.map(str::to_owned),
        size: Size::try_new(3, "<wide>".to_owned()).unwrap(),
        ratio: 0.25,
        placed_at: Instant::parse("2026-10-11T09:30:15.5Z").unwrap(),
    }
}

#[tokio::test]
async fn every_kind_of_value_round_trips_through_sqlite() {
    let db = Db::new(Sqlite::memory().unwrap());
    db.schema().migrate(&[PARCELS]).await.unwrap();
    let mut cheap = parcel(500, None);
    let mut dear = parcel(14_990, Some("fragile"));
    db.commit(&mut dear).await.unwrap();
    db.commit(&mut cheap).await.unwrap();
    dear.status = Status::Paid;
    dear.discount = Some(euros(1_000));
    db.commit(&mut dear).await.unwrap();

    let read = Parcel::query()
        .order_by(Parcel::TOTAL.asc())
        .all(&db.read())
        .await
        .unwrap();

    let ids: Vec<_> = read.iter().map(sofie::db::Aggregate::id).collect();
    assert_eq!(ids, [cheap.id(), dear.id()]);
    let dear_read = &read[1];
    assert_eq!(dear_read.status, Status::Paid);
    assert_eq!(dear_read.total, euros(14_990));
    assert_eq!(dear_read.discount, Some(euros(1_000)));
    assert_eq!(dear_read.note.as_deref(), Some("fragile"));
    assert_eq!(dear_read.size, dear.size);
    assert!((dear_read.ratio - 0.25).abs() < f64::EPSILON);
    assert_eq!(dear_read.placed_at, dear.placed_at);
    assert_eq!(read[0].discount, None);
}

#[cfg(feature = "search")]
mod search {
    use sofie::Aggregate;
    use sofie::db::{Db, File, Meta, Migrations, Searches as _, Select};
    use sofie::sqlite::Sqlite;

    #[derive(Aggregate)]
    #[aggregate(behaviors(Searched))]
    struct Label {
        meta: Meta,
        text: String,
    }

    const LABELS: Migrations = Migrations::new("on_sqlite", &[File::new("0001_labels.sql", b"")])
        .sqlite(&[File::new(
            "0001_labels.sql",
            b"CREATE TABLE on_sqlite_label (
                id uuid PRIMARY KEY,
                version bigint NOT NULL,
                created_at timestamptz NOT NULL,
                updated_at timestamptz NOT NULL,
                search_text text NOT NULL DEFAULT '',
                text text NOT NULL
            )",
        )]);

    #[tokio::test]
    async fn a_searched_percent_or_underscore_matches_only_itself() {
        let db = Db::new(Sqlite::memory().unwrap());
        db.schema().migrate(&[LABELS]).await.unwrap();
        for text in ["50% off", "500 off", "a_b", "acb"] {
            let mut label = Label {
                meta: Meta::new().unwrap(),
                text: text.to_owned(),
            };
            db.commit(&mut label).await.unwrap();
        }

        for (words, only) in [("50%", "50% off"), ("a_b", "a_b")] {
            let found: Vec<String> = Label::query()
                .where_(Label::matching(words))
                .all(&db.read())
                .await
                .unwrap()
                .into_iter()
                .map(|label| label.text)
                .collect();
            assert_eq!(found, [only], "searching {words:?}");
        }
    }
}
