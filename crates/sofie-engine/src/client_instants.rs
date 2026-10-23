use std::collections::HashSet;

use serde_json::{Map, Value};
use sofie_core::Shown;

use crate::render::items_of;

/// Where the data a client area is sent holds instants, which its JSON writes as text: the sent
/// data's shape with each instant `true`, a list standing for all its items, `null` where none
/// is. Only an instant sent as text is marked, so a withheld one's name is not sent either.
#[must_use]
pub fn instant_fields(data: Shown<'_>, sent: &Value) -> Value {
    shape(data, sent).unwrap_or(Value::Null)
}

fn shape(shown: Shown<'_>, sent: &Value) -> Option<Value> {
    match (shown, sent) {
        (Shown::Instant(_), Value::String(_)) => Some(Value::Bool(true)),
        (Shown::List(_), Value::Array(sent_items)) => list_shape(shown, sent_items),
        (Shown::Record(fields), Value::Object(sent_fields)) => {
            let found: Map<String, Value> = sent_fields
                .iter()
                .filter_map(|(name, sent_field)| {
                    Some((name.clone(), shape(fields.get(name)?, sent_field)?))
                })
                .collect();
            (!found.is_empty()).then_some(Value::Object(found))
        }
        _ => None,
    }
}

fn list_shape(list: Shown<'_>, sent_items: &[Value]) -> Option<Value> {
    let mut found: Option<Value> = None;
    let mut settled = Settled::default();
    for (item, sent_item) in items_of(Some(list)).zip(sent_items) {
        if let Some(item_shape) = shape(item, sent_item) {
            found = Some(match found {
                Some(shape) => merged(shape, item_shape),
                None => item_shape,
            });
        }
        if settled.all_after(item) {
            break;
        }
    }
    found
}

/// A list's items share a type, so once each of their fields has held a value that is neither
/// empty, a record nor a list, later items mark nothing new.
#[derive(Default)]
struct Settled<'a> {
    fields: HashSet<&'a str>,
}

impl<'a> Settled<'a> {
    fn all_after(&mut self, item: Shown<'a>) -> bool {
        let Shown::Record(fields) = item else {
            return settles(item);
        };
        let mut all = true;
        for name in fields.names() {
            if self.fields.contains(name) {
                continue;
            }
            if fields.get(name).is_some_and(settles) {
                self.fields.insert(name);
            } else {
                all = false;
            }
        }
        all
    }
}

fn settles(shown: Shown<'_>) -> bool {
    !matches!(shown, Shown::Null | Shown::Record(_) | Shown::List(_))
}

fn merged(mut shape: Value, other: Value) -> Value {
    let (Value::Object(fields), Value::Object(other_fields)) = (&mut shape, other) else {
        return shape;
    };
    for (name, other_shape) in other_fields {
        let joined = match fields.remove(&name) {
            Some(own) => merged(own, other_shape),
            None => other_shape,
        };
        fields.insert(name, joined);
    }
    shape
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use serde_json::json;
    use sofie_core::show::{Fields, Items};
    use sofie_core::{Instant, Show, Shown};

    use super::instant_fields;

    struct Row {
        at: Option<Instant>,
        note: &'static str,
    }

    impl Fields for Row {
        fn name(&self, index: usize) -> Option<&str> {
            ["at", "note"].get(index).copied()
        }

        fn get(&self, name: &str) -> Option<Shown<'_>> {
            match name {
                "at" => Some(self.at.show()),
                "note" => Some(Shown::Text(self.note)),
                _ => None,
            }
        }
    }

    impl Show for Row {
        fn show(&self) -> Shown<'_> {
            Shown::Record(self)
        }
    }

    #[test]
    fn marks_the_sent_fields_any_item_holds_an_instant_in_and_never_text() {
        let at = Instant::parse("2026-10-11T22:30:00Z").unwrap();
        let rows = vec![
            Row {
                at: None,
                note: "2026-10-11T22:30:00Z",
            },
            Row {
                at: Some(at),
                note: "",
            },
        ];

        let sent = serde_json::to_value(
            rows.iter()
                .map(|row| json!({"at": row.at, "note": row.note}))
                .collect::<Vec<_>>(),
        )
        .unwrap();

        assert_eq!(instant_fields(rows.show(), &sent), json!({"at": true}));
        assert_eq!(instant_fields(rows.show(), &json!([{}, {}])), json!(null));
        let text = json!({"at": "2026-10-11T22:30:00Z"});
        assert_eq!(instant_fields(text.show(), &text), json!(null));
    }

    struct Watched {
        rows: Vec<Row>,
        asked: Cell<usize>,
    }

    impl Items for Watched {
        fn item(&self, index: usize) -> Option<Shown<'_>> {
            self.asked.set(self.asked.get().max(index + 1));
            self.rows.get(index).map(Show::show)
        }
    }

    #[test]
    fn stops_once_every_field_of_the_items_has_shown_what_it_holds() {
        let at = Instant::parse("2026-10-11T22:30:00Z").unwrap();
        let row = |at| Row { at, note: "n" };
        let watched = Watched {
            rows: vec![row(None), row(Some(at)), row(Some(at)), row(None)],
            asked: Cell::new(0),
        };
        let sent = json!([{ "at": null }, { "at": "t" }, { "at": "t" }, { "at": null }]);

        assert_eq!(
            instant_fields(Shown::List(&watched), &sent),
            json!({ "at": true })
        );
        assert_eq!(watched.asked.get(), 2);
    }

    struct Owned {
        at: Option<Instant>,
        owner: Row,
    }

    impl Fields for Owned {
        fn name(&self, index: usize) -> Option<&str> {
            ["at", "owner"].get(index).copied()
        }

        fn get(&self, name: &str) -> Option<Shown<'_>> {
            match name {
                "at" => Some(self.at.show()),
                "owner" => Some(self.owner.show()),
                _ => None,
            }
        }
    }

    impl Show for Owned {
        fn show(&self) -> Shown<'_> {
            Shown::Record(self)
        }
    }

    #[test]
    fn walks_on_while_an_item_holds_a_nested_record() {
        let at = Instant::parse("2026-10-11T22:30:00Z").unwrap();
        let owned = |owner_at| Owned {
            at: Some(at),
            owner: Row {
                at: owner_at,
                note: "n",
            },
        };
        let rows = vec![owned(None), owned(None), owned(Some(at))];
        let sent = json!([
            { "at": "t", "owner": { "at": null } },
            { "at": "t", "owner": { "at": null } },
            { "at": "t", "owner": { "at": "t" } },
        ]);

        assert_eq!(
            instant_fields(rows.show(), &sent),
            json!({ "at": true, "owner": { "at": true } })
        );
    }
}
