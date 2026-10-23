use std::borrow::Cow;

use sofie::core::Error;
use sofie::db::{Input, lone_value, optional_lone_value};
use sofie::serde_json::json;
use sofie::{Input, Value};

#[derive(Debug, PartialEq, Value)]
struct CancelReason(String);

impl CancelReason {
    fn try_new(text: String) -> sofie::core::Result<Self> {
        if text.len() > 200 {
            return Err(Error::invalid("reason", "is too long"));
        }
        Ok(Self(text))
    }
}

#[derive(Debug, Input)]
struct NewProduct {
    #[required]
    #[max(12)]
    name: String,
    #[length(min = 2, max = 4)]
    code: CancelReason,
    #[range(min = 1, max = 99)]
    stock: i64,
    #[min(0)]
    #[unique]
    shelf: Option<i32>,
    #[required]
    note: Option<String>,
}

type Refusal = (String, &'static str, Vec<(&'static str, String)>);

fn refusals(refused: Error) -> Vec<Refusal> {
    let Error::Invalid(errors) = refused else {
        panic!("expected invalid input, got {refused:?}");
    };
    errors
        .into_iter()
        .map(|error| {
            let arguments = error
                .arguments
                .into_iter()
                .map(|(name, value)| (declared(name), value))
                .collect();
            (error.field, declared(error.key), arguments)
        })
        .collect()
}

fn declared(text: Cow<'static, str>) -> &'static str {
    match text {
        Cow::Borrowed(text) => text,
        Cow::Owned(text) => panic!("`{text}` is not a key declared in code"),
    }
}

#[test]
fn input_holds_every_field_to_its_constraints() {
    let posted = json!({ "name": "  ", "code": "ABCDE", "stock": "100", "shelf": -1 });

    let refused = NewProduct::from_json(posted).unwrap_err();

    assert_eq!(
        refusals(refused),
        [
            ("name".to_owned(), "validation.required", vec![]),
            (
                "code".to_owned(),
                "validation.max_length",
                vec![("max", "4".to_owned())]
            ),
            (
                "stock".to_owned(),
                "validation.max",
                vec![("max", "99".to_owned())]
            ),
            (
                "shelf".to_owned(),
                "validation.min",
                vec![("min", "0".to_owned())]
            ),
            ("note".to_owned(), "validation.required", vec![]),
        ]
    );
}

#[test]
fn input_within_its_constraints_is_read() {
    let posted = json!({ "name": "Desk", "code": "DS", "stock": 3, "note": "oak" });

    let product = NewProduct::from_json(posted).unwrap();

    assert_eq!(product.name, "Desk");
    assert_eq!(product.code, CancelReason("DS".to_owned()));
    assert_eq!((product.stock, product.shelf), (3, None));
    assert_eq!(product.note.as_deref(), Some("oak"));
}

#[test]
fn a_bound_names_its_unit_in_english() {
    let refused = NewProduct::from_json(
        json!({ "name": "A very long name", "code": "DS", "stock": 0, "note": "x" }),
    )
    .unwrap_err();

    assert_eq!(
        refused.to_string(),
        "invalid input: name: must be at most 12 characters; stock: must be at least 1;"
    );
}

#[test]
fn a_lone_value_is_read_from_the_field_named_after_its_type() {
    let reason: CancelReason = lone_value(json!({ "cancel_reason": "changed my mind" })).unwrap();

    assert_eq!(reason, CancelReason("changed my mind".to_owned()));
}

#[test]
fn a_lone_value_is_refused_as_an_input_field_is() {
    let missing = lone_value::<CancelReason>(json!({})).unwrap_err();
    let long = json!({ "cancel_reason": "x".repeat(201), "note": "y" });
    let invalid = lone_value::<CancelReason>(long).unwrap_err();

    assert_eq!(
        refusals(missing),
        [("cancel_reason".to_owned(), "validation.required", vec![])]
    );
    assert_eq!(
        refusals(invalid),
        [
            ("cancel_reason".to_owned(), "validation.invalid", vec![]),
            ("note".to_owned(), "validation.unknown", vec![]),
        ]
    );
}

#[test]
fn an_optional_lone_value_may_be_left_out() {
    let absent: Option<CancelReason> = optional_lone_value(json!({})).unwrap();
    let given: Option<CancelReason> =
        optional_lone_value(json!({ "cancel_reason": "late" })).unwrap();

    assert_eq!(absent, None);
    assert_eq!(given, Some(CancelReason("late".to_owned())));
}

#[cfg(feature = "factories")]
mod factories {
    use sofie::db::{Aggregate, Create, Id, Input, Meta, Update};
    use sofie::serde_json::json;
    use sofie::{Aggregate, Enum};

    #[derive(Aggregate)]
    #[owner]
    struct Shelf {
        meta: Meta,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Enum)]
    enum Finish {
        Oak,
        Walnut,
    }

    #[derive(Aggregate)]
    struct Product {
        meta: Meta,
        #[owner]
        shelf: Id<Shelf>,
        #[max(20)]
        name: String,
        finish: Finish,
    }

    #[test]
    fn an_aggregate_without_a_factory_is_created_from_its_fields_and_the_owner() {
        let shelf = Id::<Shelf>::from_uuid(Meta::new().unwrap().id());
        let input = CreateProduct::from_json(json!({ "name": "Desk", "finish": "OAK" })).unwrap();

        let product = Product::create(input, shelf).unwrap();

        assert_eq!(
            (product.name.as_str(), product.finish),
            ("Desk", Finish::Oak)
        );
        assert_eq!(product.owner(), Some(shelf.uuid()));
    }

    #[test]
    fn the_derived_update_applies_every_field_and_keeps_the_owner() {
        let shelf = Id::<Shelf>::from_uuid(Meta::new().unwrap().id());
        let create = CreateProduct::from_json(json!({ "name": "Desk", "finish": "OAK" })).unwrap();
        let mut product = Product::create(create, shelf).unwrap();
        let changes =
            UpdateProduct::from_json(json!({ "name": "Table", "finish": "WALNUT" })).unwrap();

        product.update(changes).unwrap();

        assert_eq!(
            (product.name.as_str(), product.finish),
            ("Table", Finish::Walnut)
        );
        assert_eq!(product.owner(), Some(shelf.uuid()));
    }

    #[test]
    fn the_input_of_a_derived_factory_keeps_the_schema_constraints_and_refuses_the_owner() {
        let posted = json!({ "name": "x".repeat(21), "finish": "OAK", "shelf": "?" });

        let Err(refused) = CreateProduct::from_json(posted) else {
            panic!("expected the input to be refused");
        };

        assert_eq!(
            refused.to_string(),
            "invalid input: name: must be at most 20 characters; shelf: is not accepted;"
        );
    }
}

mod derived {
    use sofie::core::{Error, FieldError};
    use sofie::db::{Input, Meta, Validate};
    use sofie::serde_json::json;
    use sofie::values::{Constraint, Control, FormFields};
    use sofie::{Aggregate, Enum, Input};

    use super::refusals;

    #[derive(Debug, Clone, Copy, PartialEq, Enum)]
    enum Finish {
        Oak,
        Walnut,
    }

    #[derive(Aggregate)]
    struct Product {
        meta: Meta,
        #[required]
        #[max(20)]
        name: String,
        #[range(min = 1, max = 9)]
        stock: i64,
        #[max(30)]
        note: String,
        finish: Finish,
    }

    #[derive(Debug, Input)]
    #[input(from = Product, validate)]
    struct Restock {
        name: String,
        #[max(200)]
        stock: i64,
        #[unconstrained]
        note: String,
        finish: Finish,
    }

    impl Validate for Restock {
        fn validate(&self) -> Result<(), Vec<FieldError>> {
            if self.finish == Finish::Walnut && self.stock > 100 {
                return Err(vec![FieldError::new(
                    "stock",
                    "validation.walnut",
                    "walnut is short",
                )]);
            }
            Ok(())
        }
    }

    #[test]
    fn a_field_inherits_its_aggregate_s_constraints_unless_it_replaces_or_drops_them() {
        let posted = json!({ "name": "x".repeat(21), "stock": 150, "note": "y".repeat(40), "finish": "OAK" });

        let refused = Restock::from_json(posted).unwrap_err();

        assert_eq!(
            refusals(refused),
            [(
                "name".to_owned(),
                "validation.max_length",
                vec![("max", "20".to_owned())]
            )]
        );
        let accepted =
            json!({ "name": "Desk", "stock": 150, "note": "y".repeat(40), "finish": "OAK" });
        let restock = Restock::from_json(accepted).unwrap();
        assert_eq!(
            (restock.name.as_str(), restock.stock, restock.note.len()),
            ("Desk", 150, 40)
        );
    }

    #[test]
    fn the_input_s_own_rules_run_after_its_fields_are_read() {
        let posted = json!({ "name": "Desk", "stock": 150, "note": "", "finish": "WALNUT" });

        let refused = Restock::from_json(posted).unwrap_err();

        assert_eq!(
            refusals(refused),
            [("stock".to_owned(), "validation.walnut", vec![])]
        );
        let Err(Error::Invalid(errors)) =
            Restock::from_json(json!({ "name": " ", "stock": 1, "note": "", "finish": "WALNUT" }))
        else {
            panic!("expected the blank name to be refused");
        };
        assert_eq!(errors[0].key, "validation.required");
    }

    #[test]
    fn a_form_shows_the_effective_constraints_and_each_value_s_control() {
        let fields = Restock::FIELDS;

        let constraints: Vec<(&str, &[Constraint])> = fields
            .iter()
            .map(|field| (field.name, field.constraints))
            .collect();
        assert_eq!(
            constraints,
            [
                (
                    "name",
                    &[Constraint::Required, Constraint::MaxLength(20)][..]
                ),
                ("stock", &[Constraint::Max(200)][..]),
                ("note", &[][..]),
                ("finish", &[][..]),
            ]
        );
        assert_eq!(fields[3].input.control, Control::Select);
        assert_eq!(fields[3].input.options, ["OAK", "WALNUT"]);
    }
}
