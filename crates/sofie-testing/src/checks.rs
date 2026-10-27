use std::fmt::Debug;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value as Json;
use sofie_core::{
    Cast, Constraint, Error, FieldError, FieldRules, RowReader, SqlType, SqlValue, Variants,
};
use sofie_db::{Aggregate, Create, Id, Input, Load};
use sofie_values::{Fixture, InputFixture, Samples, Seed};

use crate::TestApp;
use crate::fixture::{FixtureOwner, fixture_input, fixture_value};

const ROUND_TRIPS: usize = 3;

/// A bound counts characters of text and the value of a number.
#[must_use]
pub fn describe(constraint: Constraint) -> String {
    match constraint {
        Constraint::Required => "required".to_owned(),
        Constraint::Unique => "unique".to_owned(),
        Constraint::MinLength(bound) | Constraint::Min(bound) => format!("min {bound}"),
        Constraint::MaxLength(bound) | Constraint::Max(bound) => format!("max {bound}"),
    }
}

pub fn assert_moves<E: Variants>(expected: &[(&str, &[&str])]) {
    assert_eq!(
        moves::<E>(),
        expected
            .iter()
            .map(|(from, to)| (
                (*from).to_owned(),
                to.iter().map(|to| (*to).to_owned()).collect()
            ))
            .collect::<Vec<(String, Vec<String>)>>(),
        "{}'s transitions changed",
        E::TYPE
    );
    for (_, from) in E::VARIANTS {
        for (_, to) in E::VARIANTS {
            let listed = E::TRANSITIONS
                .iter()
                .any(|(source, target)| source.name() == from.name() && target.name() == to.name());
            let refusal = from.to(*to).err();
            assert_eq!(
                refusal.is_none(),
                listed,
                "{} from {} to {}: {refusal:?}",
                E::TYPE,
                from.name(),
                to.name()
            );
        }
    }
}

fn moves<E: Variants>() -> Vec<(String, Vec<String>)> {
    E::VARIANTS
        .iter()
        .map(|(name, from)| {
            let targets: Vec<String> = E::TRANSITIONS
                .iter()
                .filter(|(source, _)| source.name() == from.name())
                .map(|(_, target)| target.name().to_owned())
                .collect();
            ((*name).to_owned(), targets)
        })
        .filter(|(_, targets)| !targets.is_empty())
        .collect()
}

pub fn assert_boundary<T: Fixture + Serialize + DeserializeOwned>(
    field: &str,
    rules: FieldRules<T>,
    constraint: &str,
) {
    let declared: Vec<String> = rules.constraints.iter().copied().map(describe).collect();
    let Some(kept) = rules
        .constraints
        .iter()
        .copied()
        .find(|kept| describe(*kept) == constraint)
    else {
        panic!("`{field}` no longer keeps `{constraint}`; it keeps {declared:?}");
    };
    let (key, accepted, refused) = boundary(kept, &fixture_json::<T>());
    let refusal = |json: &Json| {
        let value: T = serde_json::from_value(json.clone())
            .unwrap_or_else(|error| panic!("`{field}` cannot hold {json} to test it: {error}"));
        rules
            .refusal(field, &value)
            .map(|error: FieldError| error.key)
    };
    if let Some(accepted) = accepted {
        assert_ne!(
            refusal(&accepted),
            Some(key.into()),
            "`{field}` refuses {accepted} at its bound"
        );
    }
    assert_eq!(
        refusal(&refused),
        Some(key.into()),
        "`{field}` accepts {refused} past its bound"
    );
}

fn fixture_json<T: Fixture + Serialize>() -> Json {
    serde_json::to_value(T::fixture(&mut Seed::new(0))).expect("a fixture serializes to JSON")
}

fn boundary(constraint: Constraint, fixture: &Json) -> (&'static str, Option<Json>, Json) {
    let text = |length: i64| -> Json {
        let Json::String(sample) = fixture else {
            panic!("a length is a bound of text, and the fixture is {fixture}");
        };
        let length = usize::try_from(length).unwrap_or_default();
        Json::String(
            sample
                .chars()
                .chain(std::iter::repeat('x'))
                .take(length)
                .collect(),
        )
    };
    match constraint {
        Constraint::Required => ("validation.required", Some(fixture.clone()), Json::from("")),
        Constraint::MinLength(bound) => {
            ("validation.min_length", Some(text(bound)), text(bound - 1))
        }
        Constraint::MaxLength(bound) => {
            ("validation.max_length", Some(text(bound)), text(bound + 1))
        }
        Constraint::Min(bound) => (
            "validation.min",
            Some(Json::from(bound)),
            Json::from(bound - 1),
        ),
        Constraint::Max(bound) => (
            "validation.max",
            Some(Json::from(bound)),
            Json::from(bound + 1),
        ),
        Constraint::Unique => panic!("only the database holds `unique`: use `assert_unique`"),
    }
}

pub async fn assert_value<V>(app: &TestApp)
where
    V: Fixture + Samples + Cast + Send + Serialize + DeserializeOwned + PartialEq + Debug,
{
    for accepted in V::ACCEPTED {
        let parsed = serde_json::from_str::<V>(accepted);
        assert!(parsed.is_ok(), "{accepted} is refused: {parsed:?}");
    }
    for refused in V::REFUSED {
        let parsed = serde_json::from_str::<V>(refused);
        assert!(parsed.is_err(), "{refused} is accepted: {parsed:?}");
    }
    for _ in 0..ROUND_TRIPS {
        let value: V = fixture_value(app);
        let json = serde_json::to_value(&value).expect("a value serializes to JSON");
        let read: V = serde_json::from_value(json.clone())
            .unwrap_or_else(|error| panic!("{json} does not read back: {error}"));
        assert_eq!(read, value, "through JSON as {json}");
        assert_eq!(
            through_database(app, &value).await,
            value,
            "through the database"
        );
    }
}

async fn through_database<V: Cast + Send>(app: &TestApp, value: &V) -> V {
    let mut params = Vec::new();
    value.encode(&mut params);
    let columns: Vec<String> = V::COLUMNS
        .iter()
        .enumerate()
        .map(|(index, column)| format!("${}::{}", index + 1, sql_type(column.ty)))
        .collect();
    let sql = format!("SELECT {}", columns.join(", "));
    let types = sofie_db::column_types(&[V::COLUMNS]);
    let mut decoded = sofie_db::query_decoded(&app.postgres, &sql, &params, types, V::decode)
        .await
        .unwrap_or_else(|error| panic!("decoding what `{sql}` gave back: {error:?}"));
    decoded.remove(0)
}

pub async fn assert_schema<A: Aggregate>(app: &TestApp) {
    let mismatches = app
        .db()
        .schema()
        .mismatches::<A>()
        .await
        .unwrap_or_else(|error| panic!("reading the table of {}: {error:?}", A::NAME));
    let described: Vec<String> = mismatches.iter().map(ToString::to_string).collect();
    assert!(
        described.is_empty(),
        "the table of {} disagrees with its schema:\n{}",
        A::NAME,
        described.join("\n")
    );
}

/// `second`, in the same state as `first`, gives its fields to `first` and is deleted, freeing its unique values.
pub async fn assert_store<A: Aggregate>(app: &TestApp, first: Id<A>, second: Id<A>) {
    let read = app.db().read();
    let loaded = A::find(&read, first)
        .await
        .expect("loading the first record");
    let other = A::find(&read, second)
        .await
        .expect("loading the second record");
    let table = format!("\"{}\".\"{}\"", A::MODULE, A::NAME);
    app.query(
        &format!("DELETE FROM {table} WHERE id = $1"),
        &[SqlValue::Uuid(second.uuid())],
    )
    .await;
    let mut changed = with_fields_of(&loaded, &other);
    app.db()
        .commit(&mut changed)
        .await
        .unwrap_or_else(|error| panic!("saving a {}: {error:?}", A::NAME));

    let reloaded = A::find(&read, first)
        .await
        .expect("loading the saved record");
    assert_eq!(
        fields(&reloaded),
        fields(&other),
        "{} loads as saved",
        A::NAME
    );
    assert_eq!(reloaded.meta().version(), loaded.meta().version() + 1);
    let mut stale = with_fields_of(&loaded, &other);
    let conflict = app.db().commit(&mut stale).await;
    assert!(
        matches!(conflict, Err(Error::Conflict)),
        "a {} saved from a stale version: {conflict:?}",
        A::NAME
    );
}

pub async fn assert_unique<A>(app: &TestApp, field: &str)
where
    A: Aggregate + Create<Owner: FixtureOwner + Send, Input: InputFixture>,
{
    let value = fixture_input::<A::Input>(app)[field].clone();
    let _first: A = app.fixture::<A>().with(field, &value).await;
    let mut input = fixture_input::<A::Input>(app);
    input[field] = value;
    let input = A::Input::from_json(input).expect("a fixture input reads");
    let mut second = A::create(input, A::Owner::fixture_owner(app)).expect("the factory builds it");
    match app.db().commit(&mut second).await {
        Err(Error::Invalid(errors))
            if errors
                .iter()
                .any(|error| error.field == field && error.key == "validation.unique") => {}
        other => panic!("a second {} with the same `{field}`: {other:?}", A::NAME),
    }
}

#[cfg(feature = "contracts")]
pub async fn assert_lookup<A, D>(app: &TestApp, ids: &[Id<A>], exposed: &[&str])
where
    A: Aggregate + Serialize,
    D: DeserializeOwned + Serialize + Send,
{
    use sofie_db::{Exposed, Lookup};

    let uuids: Vec<_> = ids.iter().map(|id| id.uuid()).collect();
    let found = Exposed::<A, D>::new(app.db().clone())
        .get_many(&uuids)
        .await
        .expect("looking the records up");
    assert_eq!(found.len(), ids.len(), "every {} is found", A::NAME);
    let mut expected: Vec<&str> = exposed.to_vec();
    expected.sort_unstable();
    for dto in found {
        let Json::Object(fields) = serde_json::to_value(dto).expect("a DTO serializes") else {
            panic!("a DTO of {} is an object", A::NAME);
        };
        let names: Vec<&str> = fields.keys().map(String::as_str).collect();
        assert_eq!(names, expected, "the fields a {} lookup answers", A::NAME);
    }
}

fn fields<A: Aggregate>(record: &A) -> Vec<SqlValue> {
    let mut fields = Vec::new();
    record.encode(&mut fields);
    fields
}

/// The children come back empty and unchanged, so saving the rebuilt record leaves the
/// stored child rows alone.
fn with_fields_of<A: Aggregate>(record: &A, other: &A) -> A {
    let mut row = fields(other);
    row.extend((0..sofie_db::child_tables::<A>()).map(|_| SqlValue::Json(Json::Array(Vec::new()))));
    A::decode(record.meta().clone(), &mut RowReader::new(&row))
        .unwrap_or_else(|error| panic!("rebuilding a {}: {error:?}", A::NAME))
}

fn sql_type(ty: SqlType) -> &'static str {
    match ty {
        SqlType::Bool => "boolean",
        SqlType::Int => "bigint",
        SqlType::Float => "double precision",
        SqlType::Text => "text",
        SqlType::Bytes => "bytea",
        SqlType::Uuid => "uuid",
        SqlType::Timestamp => "timestamptz",
        SqlType::Date => "date",
        SqlType::DateTime => "timestamp",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_constraint_reads_as_its_fact() {
        assert_eq!(describe(Constraint::MaxLength(120)), "max 120");
        assert_eq!(describe(Constraint::Min(0)), "min 0");
        assert_eq!(describe(Constraint::Required), "required");
    }

    #[test]
    fn text_is_held_to_its_length_and_a_number_to_its_range() {
        const NAME: FieldRules<String> =
            FieldRules::new(&[Constraint::Required, Constraint::MaxLength(5)]);
        const COUNT: FieldRules<i64> = FieldRules::new(&[Constraint::Min(1), Constraint::Max(9)]);

        assert_boundary("name", NAME, "required");
        assert_boundary("name", NAME, "max 5");
        assert_boundary("count", COUNT, "min 1");
        assert_boundary("count", COUNT, "max 9");
    }

    #[test]
    #[should_panic(expected = "no longer keeps `max 6`")]
    fn a_changed_bound_fails_the_test_of_the_old_one() {
        const NAME: FieldRules<String> = FieldRules::new(&[Constraint::MaxLength(5)]);

        assert_boundary("name", NAME, "max 6");
    }
}
