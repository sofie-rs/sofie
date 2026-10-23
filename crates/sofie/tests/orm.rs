#![cfg(all(
    feature = "children",
    feature = "codes",
    feature = "flags",
    feature = "history",
    feature = "joins",
    feature = "publishing"
))]

use sofie::core::{RowReader, SqlValue, Timestamp};
use sofie::db::{Aggregate, Outcome, Source, Uuid};
use sofie::prelude::*;

#[derive(Aggregate)]
#[owner]
pub struct Shop {
    meta: Meta,
    title: String,
}

#[derive(Aggregate)]
#[aggregate(behaviors(SoftDelete, Default, Slugged(from = name), UniqueCode, Audited))]
pub struct WishList {
    meta: Meta,
    #[owner]
    shop: Id<Shop>,
    name: String,
    items: Children<WishItem>,
}

#[derive(Child)]
pub struct WishItem {
    id: Id<WishItem>,
    #[length(max = 80)]
    product: String,
}

#[derive(Aggregate)]
#[owner(through = wish_list)]
#[aggregate(behaviors(RecordsChanges, PublishLock, Broadcasts, Active))]
pub struct Note {
    meta: Meta,
    wish_list: Id<WishList>,
    text: String,
}

#[derive(ReadModel)]
#[read(from = WishList)]
pub struct WishListRow {
    name: String,
    created_at: Instant,
    #[read(via = WishList::SHOP, field = Shop::TITLE)]
    shop_title: Option<String>,
}

#[test]
fn behaviors_with_arguments_and_children_describe_the_table() {
    let selected: Vec<String> = <WishList as Source>::columns()
        .iter()
        .map(|selected| format!("{selected:?}"))
        .collect();

    assert_eq!(WishList::BEHAVIORS.len(), 6);
    assert_eq!(WishList::CHILDREN[0].name, "wish_item");
    assert!(
        selected.last().unwrap().starts_with("Selected(Children("),
        "{selected:?}"
    );
    assert_eq!(Note::OWNER, Some("wish_list"));
    assert!(Note::owned_by(Uuid::from_u128(1)).is_some());
}

#[test]
fn a_joined_read_model_replies_its_creation_time() {
    let created = Timestamp::from_second(1_760_000_000).unwrap();
    let values = vec![
        SqlValue::Text("Gifts".to_owned()),
        SqlValue::Timestamp(created),
        SqlValue::Text("Corner shop".to_owned()),
    ];
    let mut row = RowReader::new(&values);

    let read = WishListRow::decode(&mut row).unwrap();

    assert_eq!(WishListRow::columns().len(), 3);
    assert_eq!(
        read.reply().unwrap().into_json().unwrap(),
        sofie::serde_json::json!({
            "name": "Gifts",
            "created_at": "2025-10-09T08:53:20Z",
            "shop_title": "Corner shop",
        })
    );
}
