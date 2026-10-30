//! List queries such as `?page=2&sort=-total&status=PAID&total.gt=100`.

use std::borrow::Cow;

use serde_json::{Map, Value, json};
use sofie_core::listing::{
    CONTROLS, DEFAULT_SIZE, ERRORS, FILTER, HAS_NEXT, ITEMS, PAGE, QUERY, REFUSAL_ARGUMENTS,
    REFUSAL_KEY, REFUSAL_MESSAGE, SIZE, SORT,
};
use sofie_core::show::Fields;
use sofie_core::{Error, FieldError, Result, Show, Shown};
use sofie_db::{
    Aggregate, Condition, FieldNames, Input, Outcome, Page, Paginated, Query, Read, ReadOnly,
    Reply, ReplyData, Select, Sort, Source, filterable, selects,
};
use sofie_values::Operator;

use crate::route_path::same_text;
use crate::urlencoded::decode;
use crate::{Area, RequestContext};
use sofie_db::Resolve;

/// Read by the listing itself, so no field of its query `Q` may take one.
const RESERVED: [&str; 4] = [PAGE, SIZE, SORT, FILTER];
/// Each set of filters is its own SQL text and so its own prepared statement; with no field and
/// operator twice and a fixed order, a caller cannot make them without end.
const MAX_FILTERS: usize = 16;

/// A path parameter naming a field filters too, so a child is listed only under its own parent.
/// `Q` reads the query's other parameters, as [`crate::Query`] does; `()` reads none.
pub struct Listing<T: Source, Q = ()> {
    narrowed: Query<T>,
    page: Page,
    sort: Option<String>,
    filters: Vec<Filter>,
    parents: Vec<(String, String)>,
    prefers_json: bool,
    parameters: Option<Q>,
    asked_parameters: Map<String, Value>,
    refused_parameters: Vec<FieldError>,
}

struct Filter {
    key: String,
    field: String,
    operator: Operator,
    value: String,
}

impl<T: Source, Q> Listing<T, Q> {
    /// None when a value was refused: the page then lists nothing and names the refusal.
    pub fn parameters(&self) -> Option<&Q> {
        self.parameters.as_ref()
    }

    #[must_use]
    pub fn where_(mut self, condition: Condition<T::Base>) -> Self {
        self.narrowed = self.narrowed.where_(condition);
        self
    }

    pub fn query(&self) -> Result<Query<T>> {
        let (query, refused) = self.filtered()?;
        if refused.is_empty() {
            return Ok(query);
        }
        Err(Error::Invalid(refused))
    }

    /// A value its field refuses is the asker's to correct, under the filter's key.
    fn filtered(&self) -> Result<(Query<T>, Vec<FieldError>)> {
        let (mut query, mut refused) = (self.narrowed.clone(), self.refused_parameters.clone());
        for filter in &self.filters {
            match Condition::on_field(&filter.field, filter.operator, &filter.value) {
                Ok(condition) => query = query.where_(condition),
                Err(Error::Invalid(errors)) => {
                    refused.extend(errors.into_iter().map(|error| FieldError {
                        field: filter.key.clone(),
                        ..error
                    }));
                }
                Err(error) => return Err(error),
            }
        }
        for (field, id) in &self.parents {
            let parent =
                Condition::on_field(field, Operator::Equals, id).map_err(|error| match error {
                    Error::Invalid(_) => Error::NotFound,
                    other => other,
                })?;
            query = query.where_(parent);
        }
        if let Some(sort) = &self.sort {
            let (field, descending) = sort_field(sort);
            let sort = Sort::by_field(field, descending).ok_or_else(|| {
                Error::internal_message(format!("sorting by `{field}`, which was checked"))
            })?;
            query = query.order_by(sort);
        }
        Ok((query, refused))
    }

    /// A parent beyond the read's reach is not found: an empty list would say it exists. A
    /// filter in error lists nothing, so no one reads an unfiltered list as a filtered one; a
    /// JSON caller or route gets the errors as a 422, as [`Self::query`] answers.
    pub async fn page(self, read: &Read) -> Result<Listed<T>> {
        #[cfg(feature = "groups")]
        for (field, id) in &self.parents {
            if let Ok(parent) = id.parse() {
                sofie_db::refuse_parent_beyond_reach::<T::Base>(read, field, parent).await?;
            }
        }
        let (query, refused) = self.filtered()?;
        if self.prefers_json && !refused.is_empty() {
            return Err(Error::Invalid(refused));
        }
        let page = if refused.is_empty() {
            query.paginate(read, self.page).await?
        } else {
            Paginated::new(Vec::new(), self.page, false)
        };
        Ok(Listed {
            page,
            controls: controls::<T>(),
            sort: self.sort,
            filter: self
                .filters
                .into_iter()
                .map(|filter| (filter.key, Value::String(filter.value)))
                .collect(),
            query: self.asked_parameters,
            errors: Refused(refused.into_iter().map(RefusedFilter::from).collect()),
        })
    }
}

/// What a filter form offers for a field: an enum's variants, or a picker for an instant.
fn controls<T: Source>() -> Map<String, Value> {
    T::Base::FIELDS
        .iter()
        .filter(|field| selects::<T>(field.name) && !filterable::<T::Base>(field.name).is_empty())
        .filter_map(|field| {
            let [column] = field.columns else {
                return None;
            };
            let control = if !column.values.is_empty() {
                json!({ "values": column.values })
            } else if column.ty == sofie_core::SqlType::Timestamp {
                json!({ "type": "datetime-local" })
            } else {
                return None;
            };
            Some((field.name.to_owned(), control))
        })
        .collect()
}

fn sort_field(sort: &str) -> (&str, bool) {
    match sort.strip_prefix('-') {
        Some(field) => (field, true),
        None => (sort, false),
    }
}

impl<A: Area, M, T: Source, Q: Input + FieldNames + Send> Resolve<RequestContext<A, M>>
    for Listing<T, Q>
{
    fn resolve(context: &mut RequestContext<A, M>) -> impl Future<Output = Result<Self>> + Send {
        let parents = context
            .matched
            .parameters
            .iter()
            .filter(|(name, _)| name != T::Base::NAME && !filterable::<T::Base>(name).is_empty())
            .cloned()
            .collect();
        let prefers_json = context.matched.prefers_json;
        std::future::ready(
            asked::<T, Q>(context.query().unwrap_or_default()).map(|mut listing| {
                listing.parents = parents;
                listing.prefers_json = prefers_json;
                listing
            }),
        )
    }
}

fn asked<T: Source, Q: Input + FieldNames>(query: &str) -> Result<Listing<T, Q>> {
    const {
        assert!(
            !names_any(Q::FIELD_NAMES, &RESERVED),
            "a listing's query struct names a field `page`, `size`, `sort` or `filter`, \
             which the listing reads itself"
        );
        assert!(
            !names_a_field::<T::Base>(Q::FIELD_NAMES),
            "a listing's query struct names a field `id` or a field of the listed aggregate, \
             which would hide the list's filter of that name; rename the query struct's field"
        );
    }
    let (mut number, mut size, mut sort, mut filters) = (1, DEFAULT_SIZE, None, Vec::new());
    let mut asked_parameters = Map::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let (key, value) = (decode(key.as_bytes())?, decode(value.as_bytes())?);
        match key.as_str() {
            PAGE => number = whole_number(&key, &value)?,
            SIZE => size = whole_number(&key, &value)?,
            SORT if sortable::<T>(sort_field(&value).0) => {
                sort = Some(value);
            }
            SORT => {
                return Err(Error::BadRequest(format!(
                    "a list cannot be sorted by `{value}`"
                )));
            }
            _ if value.is_empty() && key.strip_suffix(".empty").is_none() => {}
            _ if Q::FIELD_NAMES.contains(&parameter_name(&key)) => {
                asked_parameters.insert(key, Value::String(value));
            }
            _ if crate::query::is_tracking_key(&key) && !selects::<T>(parameter_name(&key)) => {}
            _ => add_filter(&mut filters, filter_of::<T>(key, value)?)?,
        }
    }
    filters.sort_by(|first, second| first.key.cmp(&second.key));
    let (parameters, refused_parameters) = read_parameters(&asked_parameters)?;
    Ok(Listing {
        narrowed: T::query(),
        page: Page::new(number, size)?,
        sort,
        filters,
        parents: Vec::new(),
        prefers_json: false,
        parameters,
        asked_parameters,
        refused_parameters,
    })
}

/// `budget.amount` is a part of the parameter `budget`, as `Query<Q>` reads it.
fn parameter_name(key: &str) -> &str {
    key.split_once('.').map_or(key, |(name, _)| name)
}

/// A refused value is the asker's to correct, as a refused filter is. The asked parameters stay
/// flat, as links and the filter form write them back.
fn read_parameters<Q: Input>(asked: &Map<String, Value>) -> Result<(Option<Q>, Vec<FieldError>)> {
    let mut nested = Map::new();
    for (key, value) in asked {
        let text = value.as_str().unwrap_or_default().to_owned();
        crate::form::insert(&mut nested, key, text)?;
    }
    match Q::from_json(Value::Object(nested)) {
        Ok(parameters) => Ok((Some(parameters), Vec::new())),
        Err(Error::Invalid(refused)) => Ok((None, refused)),
        Err(error) => Err(error),
    }
}

const fn names_any(names: &[&str], reserved: &[&str]) -> bool {
    let mut index = 0;
    while index < names.len() {
        let mut other = 0;
        while other < reserved.len() {
            if same_text(names[index], reserved[other]) {
                return true;
            }
            other += 1;
        }
        index += 1;
    }
    false
}

const fn names_a_field<A: Aggregate>(names: &[&str]) -> bool {
    let mut index = 0;
    while index < A::FIELDS.len() {
        if names_any(names, &[A::FIELDS[index].name]) {
            return true;
        }
        index += 1;
    }
    names_any(names, &["id"])
}

fn whole_number(key: &str, value: &str) -> Result<u64> {
    value
        .parse()
        .map_err(|_| Error::BadRequest(format!("`{key}` is a whole number, not `{value}`")))
}

fn sortable<T: Source>(field: &str) -> bool {
    selects::<T>(field) && Sort::<T::Base>::by_field(field, false).is_some()
}

fn filter_of<T: Source>(key: String, value: String) -> Result<Filter> {
    let (field, operator) = match key.split_once('.') {
        Some((field, operator)) => (field, operator_named(operator)?),
        None => (key.as_str(), Operator::Equals),
    };
    if !selects::<T>(field) || !filterable::<T::Base>(field).contains(&operator) {
        return Err(Error::BadRequest(format!(
            "a list cannot be filtered by `{key}`"
        )));
    }
    Ok(Filter {
        field: field.to_owned(),
        key,
        operator,
        value,
    })
}

fn add_filter(filters: &mut Vec<Filter>, filter: Filter) -> Result<()> {
    if filters
        .iter()
        .any(|held| held.field == filter.field && held.operator == filter.operator)
    {
        return Err(Error::BadRequest(format!(
            "a list is filtered by `{}` once at most",
            filter.key
        )));
    }
    if filters.len() == MAX_FILTERS {
        return Err(Error::BadRequest(format!(
            "a list takes at most {MAX_FILTERS} filters"
        )));
    }
    filters.push(filter);
    Ok(())
}

fn operator_named(name: &str) -> Result<Operator> {
    Operator::named(name).ok_or_else(|| {
        Error::BadRequest(format!(
            "`{name}` is no filter; filters are ne, gt, lt, gte, lte, between, contains and empty"
        ))
    })
}

/// Answered as `{items, page, size, has_next, sort, filter, query, controls, errors}`, what a `Table`
/// links from; `errors` holds each filter whose value its field refuses, with its message's key
/// and arguments, so a client area shows it in the viewer's language.
pub struct Listed<T> {
    page: Paginated<T>,
    controls: Map<String, Value>,
    sort: Option<String>,
    filter: Map<String, Value>,
    query: Map<String, Value>,
    errors: Refused,
}

/// Shown with each message's key and arguments, so a page writes it in the viewer's language.
struct Refused(Vec<RefusedFilter>);

struct RefusedFilter {
    key: String,
    message_key: Cow<'static, str>,
    arguments: RefusalArguments,
    message: String,
}

struct RefusalArguments(Vec<(Cow<'static, str>, String)>);

impl From<FieldError> for RefusedFilter {
    fn from(error: FieldError) -> Self {
        Self {
            key: error.field,
            message_key: error.key,
            arguments: RefusalArguments(error.arguments),
            message: error.message,
        }
    }
}

impl Refused {
    fn records(&self) -> Map<String, Value> {
        self.0
            .iter()
            .map(|refused| {
                let arguments: Map<String, Value> = refused
                    .arguments
                    .0
                    .iter()
                    .map(|(name, value)| (name.to_string(), Value::from(value.as_str())))
                    .collect();
                let record = json!({
                    REFUSAL_KEY: refused.message_key,
                    REFUSAL_ARGUMENTS: arguments,
                    REFUSAL_MESSAGE: refused.message,
                });
                (refused.key.clone(), record)
            })
            .collect()
    }
}

impl Fields for Refused {
    fn name(&self, index: usize) -> Option<&str> {
        self.0.get(index).map(|refused| refused.key.as_str())
    }

    fn get(&self, name: &str) -> Option<Shown<'_>> {
        let refused = self.0.iter().find(|refused| refused.key == name)?;
        Some(Shown::Record(refused))
    }
}

const REFUSAL_NAMES: [&str; 3] = [REFUSAL_KEY, REFUSAL_ARGUMENTS, REFUSAL_MESSAGE];

impl Fields for RefusedFilter {
    fn name(&self, index: usize) -> Option<&str> {
        REFUSAL_NAMES.get(index).copied()
    }

    fn get(&self, name: &str) -> Option<Shown<'_>> {
        match name {
            REFUSAL_KEY => Some(Shown::Text(&self.message_key)),
            REFUSAL_ARGUMENTS => Some(Shown::Record(&self.arguments)),
            REFUSAL_MESSAGE => Some(Shown::Text(&self.message)),
            _ => None,
        }
    }
}

impl Fields for RefusalArguments {
    fn name(&self, index: usize) -> Option<&str> {
        self.0.get(index).map(|(name, _)| name.as_ref())
    }

    fn get(&self, name: &str) -> Option<Shown<'_>> {
        let (_, value) = self.0.iter().find(|(argument, _)| *argument == name)?;
        Some(Shown::Text(value))
    }
}

impl<T: ReplyData> Listed<T> {
    #[must_use]
    pub fn items(&self) -> &[T] {
        self.page.items()
    }

    pub fn to_json(&self) -> Result<Value> {
        self.json()
            .map_err(|error| Error::internal("serializing a list's item", error))
    }

    fn json(&self) -> serde_json::Result<Value> {
        let items = self
            .items()
            .iter()
            .map(ReplyData::to_json)
            .collect::<serde_json::Result<Vec<Value>>>()?;
        Ok(json!({
            ITEMS: items,
            PAGE: self.page.page().number(),
            SIZE: self.page.page().size(),
            HAS_NEXT: self.page.has_next(),
            SORT: self.sort,
            FILTER: self.filter,
            QUERY: self.query,
            CONTROLS: self.controls,
            ERRORS: self.errors.records(),
        }))
    }
}

/// Ordered as [`Listed::json`]'s object holds its keys.
const LISTED_NAMES: [&str; 9] = [
    CONTROLS, ERRORS, FILTER, HAS_NEXT, ITEMS, PAGE, QUERY, SIZE, SORT,
];

impl<T: Show> Fields for Listed<T> {
    fn name(&self, index: usize) -> Option<&str> {
        LISTED_NAMES.get(index).copied()
    }

    fn get(&self, name: &str) -> Option<Shown<'_>> {
        match name {
            CONTROLS => Some(Shown::Record(&self.controls)),
            ERRORS => Some(Shown::Record(&self.errors)),
            FILTER => Some(Shown::Record(&self.filter)),
            QUERY => Some(Shown::Record(&self.query)),
            SORT => Some(self.sort.show()),
            _ => self.page.get(name),
        }
    }
}

impl<T: Show> Show for Listed<T> {
    fn show(&self) -> Shown<'_> {
        Shown::Record(self)
    }
}

impl<T: Show + ReplyData> ReplyData for Listed<T> {
    fn write_json(&self, out: &mut Vec<u8>) -> serde_json::Result<()> {
        serde_json::to_writer(out, &self.json()?)
    }

    fn to_json(&self) -> serde_json::Result<Value> {
        self.json()
    }

    fn shown(&self) -> Option<Shown<'_>> {
        Some(self.show())
    }
}

impl<T: Show + ReplyData + 'static> Outcome for Listed<T> {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Data(Box::new(self)))
    }
}

impl<T: Show + ReplyData + 'static> ReadOnly for Listed<T> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_order_is_a_field_name_descending_with_a_minus() {
        assert_eq!(sort_field("-total"), ("total", true));
        assert_eq!(sort_field("total"), ("total", false));
    }

    #[test]
    fn an_operator_is_named_after_the_field() {
        assert_eq!(operator_named("gt").unwrap(), Operator::GreaterThan);
        assert_eq!(operator_named("gte").unwrap(), Operator::AtLeast);
        assert_eq!(operator_named("lte").unwrap(), Operator::AtMost);
        assert!(matches!(operator_named("like"), Err(Error::BadRequest(_))));
    }

    const TEXT: &[sofie_core::ColumnSpec] =
        &[sofie_core::ColumnSpec::new("", sofie_core::SqlType::Text)];

    struct Note(sofie_db::Meta);

    impl Aggregate for Note {
        const NAME: &'static str = "note";
        const MODULE: &'static str = "shop";
        const FIELDS: &'static [sofie_db::Field] = &[
            sofie_db::Field::new("title", TEXT, true),
            sofie_db::Field::new("secret", TEXT, false),
        ];

        fn meta(&self) -> &sofie_db::Meta {
            &self.0
        }

        fn meta_mut(&mut self) -> &mut sofie_db::Meta {
            &mut self.0
        }

        fn encode(&self, _out: &mut Vec<sofie_core::SqlValue>) {}

        fn decode(meta: sofie_db::Meta, _row: &mut dyn sofie_core::ColumnReader) -> Result<Self> {
            Ok(Self(meta))
        }
    }

    #[test]
    fn an_emptiness_filter_needs_no_value_and_a_blank_filter_is_ignored() {
        let listing = asked::<Note, ()>("title.empty&title=").unwrap();

        let filters: Vec<_> = listing
            .filters
            .iter()
            .map(|filter| (filter.key.as_str(), filter.operator))
            .collect();
        assert_eq!(filters, [("title.empty", Operator::IsEmpty)]);
    }

    struct Title;

    impl Source for Title {
        type Base = Note;

        fn columns() -> Vec<sofie_db::Selected> {
            sofie_db::read_columns::<Note>(&["title"])
        }

        fn column_types() -> Vec<Option<sofie_core::SqlType>> {
            vec![Some(sofie_core::SqlType::Text)]
        }

        fn decode(_row: &mut dyn sofie_core::ColumnReader) -> Result<Self> {
            Ok(Self)
        }
    }

    const STATUS: &[sofie_core::ColumnSpec] =
        &[sofie_core::ColumnSpec::new("", sofie_core::SqlType::Text)
            .with_values(&["OPEN", "PAID"])];
    const INSTANT: &[sofie_core::ColumnSpec] = &[sofie_core::ColumnSpec::new(
        "",
        sofie_core::SqlType::Timestamp,
    )];

    struct Booking(sofie_db::Meta);

    impl Aggregate for Booking {
        const NAME: &'static str = "booking";
        const MODULE: &'static str = "shop";
        const FIELDS: &'static [sofie_db::Field] = &[
            sofie_db::Field::new("status", STATUS, false),
            sofie_db::Field::new("starts_at", INSTANT, false),
            sofie_db::Field::new("note", TEXT, false),
        ];

        fn meta(&self) -> &sofie_db::Meta {
            &self.0
        }

        fn meta_mut(&mut self) -> &mut sofie_db::Meta {
            &mut self.0
        }

        fn encode(&self, _out: &mut Vec<sofie_core::SqlValue>) {}

        fn decode(meta: sofie_db::Meta, _row: &mut dyn sofie_core::ColumnReader) -> Result<Self> {
            Ok(Self(meta))
        }
    }

    #[test]
    fn a_list_names_an_enum_s_variants_and_an_instant_for_its_filter_form() {
        assert_eq!(
            Value::Object(controls::<Booking>()),
            json!({
                "status": {"values": ["OPEN", "PAID"]},
                "starts_at": {"type": "datetime-local"},
            })
        );
    }

    #[test]
    fn a_list_takes_each_filter_once_in_one_order_and_a_bounded_number_of_them() {
        let one_way = asked::<Booking, ()>("status=OPEN&note.contains=a").unwrap();
        let other_way = asked::<Booking, ()>("note.contains=b&status=PAID").unwrap();
        let keys = |listing: &Listing<Booking>| -> Vec<String> {
            listing
                .filters
                .iter()
                .map(|filter| filter.key.clone())
                .collect()
        };
        assert_eq!(keys(&one_way), keys(&other_way));

        let repeated = asked::<Booking, ()>("status.ne=OPEN&status.ne=PAID");
        assert!(matches!(repeated, Err(Error::BadRequest(_))));
        assert!(asked::<Booking, ()>("status.ne=OPEN&status=PAID").is_ok());

        let mut filters = Vec::new();
        let added: Vec<bool> = (0..=MAX_FILTERS)
            .map(|index| {
                let filter = Filter {
                    key: format!("field_{index}"),
                    field: format!("field_{index}"),
                    operator: Operator::Equals,
                    value: "a".to_owned(),
                };
                add_filter(&mut filters, filter).is_ok()
            })
            .collect();
        assert_eq!(added.iter().filter(|added| **added).count(), MAX_FILTERS);
        assert!(!added[MAX_FILTERS]);
    }

    #[test]
    fn a_list_filters_and_sorts_only_by_what_its_read_model_selects() {
        assert!(asked::<Title, ()>("title.contains=a&sort=-title").is_ok());
        assert!(asked::<Note, ()>("secret.contains=a&sort=secret").is_ok());
        for query in [
            "secret.contains=a",
            "secret=a",
            "sort=secret",
            "sort=-secret",
        ] {
            let refused = asked::<Title, ()>(query);
            assert!(matches!(refused, Err(Error::BadRequest(_))), "{query}");
        }
    }

    struct Booked {
        at: sofie_core::Instant,
    }

    impl serde::Serialize for Booked {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.collect_map([("at", self.at)])
        }
    }

    impl sofie_core::show::Fields for Booked {
        fn name(&self, index: usize) -> Option<&str> {
            ["at"].get(index).copied()
        }

        fn get(&self, name: &str) -> Option<sofie_core::Shown<'_>> {
            (name == "at").then(|| sofie_core::Show::show(&self.at))
        }
    }

    impl sofie_core::Show for Booked {
        fn show(&self) -> sofie_core::Shown<'_> {
            sofie_core::Shown::Record(self)
        }
    }

    fn first_at(shown: sofie_core::Shown<'_>) -> Option<sofie_core::Shown<'_>> {
        let sofie_core::Shown::Record(page) = shown else {
            return None;
        };
        let sofie_core::Shown::List(items) = page.get("items")? else {
            return None;
        };
        let sofie_core::Shown::Record(item) = items.item(0)? else {
            return None;
        };
        item.get("at")
    }

    #[test]
    fn a_listed_page_replies_its_items_typed_and_its_json_unchanged() {
        let at = sofie_core::Instant::parse("2026-10-11T22:30:00Z").unwrap();
        let listed = Listed {
            page: Paginated::new(vec![Booked { at }], Page::new(1, 20).unwrap(), false),
            controls: controls::<Booking>(),
            sort: Some("-at".to_owned()),
            filter: Map::from_iter([("note".to_owned(), Value::from("a"))]),
            query: Map::new(),
            errors: Refused(Vec::new()),
        };

        let reply = listed.reply().unwrap();

        let first_at = reply.shown().and_then(first_at);
        assert!(matches!(first_at, Some(sofie_core::Shown::Instant(shown)) if shown == at));
        #[cfg(feature = "clients")]
        assert_eq!(
            sofie_engine::instant_fields(sofie_core::Show::show(&reply), &reply.to_json().unwrap()),
            json!({"items": {"at": true}})
        );
        assert_eq!(
            reply.to_json().unwrap(),
            json!({
                "items": [{"at": "2026-10-11T22:30:00Z"}],
                "page": 1,
                "size": 20,
                "has_next": false,
                "sort": "-at",
                "filter": {"note": "a"},
                "query": {},
                "controls": controls::<Booking>(),
                "errors": {},
            })
        );
    }

    #[tokio::test]
    async fn a_filter_its_field_refuses_lists_nothing_and_is_named_beside_it() {
        let db = sofie_db::Db::new(sofie_testing::FakeDriver::default());
        let read = Read::within(&db, sofie_db::Reach::All);
        let listing = asked::<Booking, ()>("starts_at.gt=yesterday&status=OPEN").unwrap();

        let listed = listing.page(&read).await.unwrap();

        assert!(listed.page.items().is_empty());
        assert_eq!(listed.filter["starts_at.gt"], "yesterday");
        let errors = listed.errors.records();
        let error = errors["starts_at.gt"][REFUSAL_MESSAGE]
            .as_str()
            .unwrap_or_default();
        assert!(error.contains("with an offset"), "{errors:?}");
        assert!(matches!(
            asked::<Booking, ()>("starts_at=yesterday").unwrap().query(),
            Err(Error::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn a_refused_filter_of_any_type_is_named_beside_it_on_a_page_and_a_422_for_json() {
        let db = sofie_db::Db::new(sofie_testing::FakeDriver::default());
        let read = Read::within(&db, sofie_db::Reach::All);
        let query = "starts_at.gt=yesterday&status=CLOSED&id=7";

        let listed = asked::<Booking, ()>(query)
            .unwrap()
            .page(&read)
            .await
            .unwrap();
        let mut json = asked::<Booking, ()>(query).unwrap();
        json.prefers_json = true;
        let refused = json.page(&read).await.err();

        let errors = listed.errors.records();
        let named: Vec<&String> = errors.keys().collect();
        assert_eq!(named, ["id", "starts_at.gt", "status"]);
        let Some(Error::Invalid(errors)) = refused else {
            panic!("{refused:?}");
        };
        let fields: Vec<&str> = errors.iter().map(|error| error.field.as_str()).collect();
        assert_eq!(fields, ["id", "starts_at.gt", "status"]);
    }

    #[tokio::test]
    async fn a_refused_filter_is_shown_with_its_message_s_key_and_arguments() {
        let db = sofie_db::Db::new(sofie_testing::FakeDriver::default());
        let read = Read::within(&db, sofie_db::Reach::All);

        let listing = asked::<Booking, ()>("status=CLOSED").unwrap();
        let listed = listing.page(&read).await.unwrap();

        let Some(Shown::Record(refused)) = listed.errors.get("status") else {
            panic!("{:?}", listed.errors.records());
        };
        assert!(matches!(
            refused.get(REFUSAL_KEY),
            Some(Shown::Text("validation.one_of"))
        ));
        let Some(Shown::Record(arguments)) = refused.get(REFUSAL_ARGUMENTS) else {
            panic!("the refusal shows no arguments");
        };
        assert!(matches!(
            arguments.get("values"),
            Some(Shown::Text("OPEN, PAID"))
        ));
        assert_eq!(
            Value::Object(listed.errors.records()),
            json!({"status": {
                "key": "validation.one_of",
                "arguments": {"values": "OPEN, PAID"},
                "message": "must be one of: OPEN, PAID",
            }})
        );
    }

    struct Around {
        near: Option<String>,
        radius: Option<u32>,
    }

    impl Input for Around {
        fn from_json(value: Value) -> Result<Self> {
            let mut fields = sofie_db::InputFields::new(value)?;
            let near = fields.optional("near");
            let radius = fields.optional::<u32>("radius");
            let radius = fields.checked(radius, |radius| {
                radius
                    .is_some_and(|radius| radius > 50)
                    .then(|| FieldError::new("radius", "validation.max", "must be at most 50"))
            });
            fields.finish()?;
            Ok(Self {
                near: near?,
                radius: radius?,
            })
        }
    }

    impl FieldNames for Around {
        const FIELD_NAMES: &'static [&'static str] = &["near", "radius"];
    }

    #[tokio::test]
    async fn a_list_reads_its_query_parameters_typed_beside_its_filters_and_narrows_by_them() {
        let driver = sofie_testing::FakeDriver::default();
        let db = sofie_db::Db::new(driver.clone());
        let read = Read::within(&db, sofie_db::Reach::All);
        let listing = asked::<Booking, Around>("near=Ulm&radius=5&status=OPEN&page=2").unwrap();
        let around = listing.parameters().unwrap();
        assert_eq!(around.radius, Some(5));
        let near = Condition::on_field("note", Operator::Equals, around.near.as_deref().unwrap());

        let listed = listing.where_(near.unwrap()).page(&read).await.unwrap();

        assert_eq!(
            Value::Object(listed.query),
            json!({"near": "Ulm", "radius": "5"})
        );
        assert_eq!(Value::Object(listed.filter), json!({"status": "OPEN"}));
        let (_, arguments) = driver.queries().pop().unwrap();
        assert!(
            arguments.contains(&sofie_core::SqlValue::Text("Ulm".to_owned())),
            "{arguments:?}"
        );
    }

    #[test]
    fn a_tracking_key_appended_to_a_shared_list_link_is_no_filter() {
        let listing =
            asked::<Booking, Around>("near=Ulm&utm_source=ad&fbclid=a&status=OPEN").unwrap();

        assert_eq!(
            listing.asked_parameters,
            json!({"near": "Ulm"}).as_object().cloned().unwrap()
        );
        assert_eq!(listing.filters.len(), 1);
        assert!(matches!(
            asked::<Booking, Around>("utm=x"),
            Err(Error::BadRequest(_))
        ));
    }

    struct Visit(sofie_db::Meta);

    impl Aggregate for Visit {
        const NAME: &'static str = "visit";
        const MODULE: &'static str = "shop";
        const FIELDS: &'static [sofie_db::Field] =
            &[sofie_db::Field::new("utm_source", TEXT, true)];

        fn meta(&self) -> &sofie_db::Meta {
            &self.0
        }

        fn meta_mut(&mut self) -> &mut sofie_db::Meta {
            &mut self.0
        }

        fn encode(&self, _out: &mut Vec<sofie_core::SqlValue>) {}

        fn decode(meta: sofie_db::Meta, _row: &mut dyn sofie_core::ColumnReader) -> Result<Self> {
            Ok(Self(meta))
        }
    }

    #[test]
    fn a_tracking_key_naming_a_listed_field_stays_a_filter() {
        let listing = asked::<Visit, ()>("utm_source=ad&utm_medium=mail&gclid=a").unwrap();

        let filters: Vec<_> = listing
            .filters
            .iter()
            .map(|filter| (filter.field.as_str(), filter.value.as_str()))
            .collect();
        assert_eq!(filters, [("utm_source", "ad")]);
    }

    #[tokio::test]
    async fn a_refused_query_parameter_is_named_beside_the_list_and_a_422_for_json() {
        let db = sofie_db::Db::new(sofie_testing::FakeDriver::default());
        let read = Read::within(&db, sofie_db::Reach::All);

        let listing = asked::<Booking, Around>("radius=80").unwrap();
        assert!(listing.parameters().is_none());
        let listed = listing.page(&read).await.unwrap();
        let mut json = asked::<Booking, Around>("radius=80").unwrap();
        json.prefers_json = true;
        let refused = json.page(&read).await.err();

        assert!(listed.page.items().is_empty());
        assert_eq!(
            Value::Object(listed.errors.records()),
            json!({"radius": {
                "key": "validation.max",
                "arguments": {},
                "message": "must be at most 50",
            }})
        );
        assert_eq!(listed.query["radius"], "80");
        assert!(matches!(refused, Some(Error::Invalid(errors)) if errors[0].field == "radius"));
        assert!(matches!(
            asked::<Booking, Around>("radius=80").unwrap().query(),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            asked::<Booking, Around>("nearby=Ulm"),
            Err(Error::BadRequest(_))
        ));
    }

    struct Budgeted {
        budget: Budget,
    }

    struct Budget {
        amount: String,
        currency: String,
    }

    impl Input for Budgeted {
        fn from_json(value: Value) -> Result<Self> {
            let mut fields = sofie_db::InputFields::new(value)?;
            let budget = fields.take::<Value>("budget");
            fields.finish()?;
            let mut budget = sofie_db::InputFields::new(budget?)?;
            let (amount, currency) = (budget.take("amount"), budget.take("currency"));
            budget.finish()?;
            Ok(Self {
                budget: Budget {
                    amount: amount?,
                    currency: currency?,
                },
            })
        }
    }

    impl FieldNames for Budgeted {
        const FIELD_NAMES: &'static [&'static str] = &["budget"];
    }

    #[test]
    fn a_nested_query_parameter_is_read_from_its_dotted_keys() {
        let listing =
            asked::<Booking, Budgeted>("budget.amount=5&budget.currency=EUR&status=OPEN").unwrap();

        let budgeted = listing.parameters().unwrap();
        assert_eq!(
            (
                budgeted.budget.amount.as_str(),
                budgeted.budget.currency.as_str()
            ),
            ("5", "EUR")
        );
        assert_eq!(
            Value::Object(listing.asked_parameters.clone()),
            json!({"budget.amount": "5", "budget.currency": "EUR"})
        );
    }

    #[test]
    fn a_query_parameter_may_not_take_a_key_the_list_reads() {
        assert!(names_any(&["near", "page"], &RESERVED));
        assert!(names_any(&["filter"], &RESERVED));
        assert!(!names_any(&["near", "pages", "query"], &RESERVED));
    }

    #[test]
    fn a_query_parameter_may_not_take_the_name_of_a_field_the_list_filters_by() {
        assert!(names_a_field::<Booking>(&["near", "status"]));
        assert!(names_a_field::<Booking>(&["id"]));
        assert!(!names_a_field::<Booking>(&["near", "radius", "notes"]));
    }

    #[tokio::test]
    async fn a_list_under_a_parent_its_field_refuses_is_not_found() {
        let db = sofie_db::Db::new(sofie_testing::FakeDriver::default());
        let mut listing = asked::<Booking, ()>("").unwrap();
        listing.parents = vec![("status".to_owned(), "CLOSED".to_owned())];

        let listed = listing.page(&Read::within(&db, sofie_db::Reach::All)).await;

        assert!(matches!(listed, Err(Error::NotFound)), "{:?}", listed.err());
    }

    #[test]
    fn a_listed_page_names_its_fields_as_its_json_holds_them() {
        let listed = Listed {
            page: Paginated::new(Vec::<Booked>::new(), Page::new(1, 20).unwrap(), false),
            controls: Map::new(),
            sort: None,
            filter: Map::new(),
            query: Map::new(),
            errors: Refused(Vec::new()),
        };

        let names: Vec<&str> = (0..).map_while(|index| listed.name(index)).collect();
        let json = listed.to_json().unwrap();
        let keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();

        assert_eq!(names, keys);
    }

    #[cfg(feature = "groups")]
    struct Folder(sofie_db::Meta);

    #[cfg(feature = "groups")]
    impl Aggregate for Folder {
        const NAME: &'static str = "folder";
        const MODULE: &'static str = "shop";
        const FIELDS: &'static [sofie_db::Field] = &[sofie_db::Field::new(
            "account",
            <sofie_core::Uuid as sofie_core::Cast>::COLUMNS,
            false,
        )];
        const OWNER: Option<&'static str> = Some("account");

        fn meta(&self) -> &sofie_db::Meta {
            &self.0
        }

        fn meta_mut(&mut self) -> &mut sofie_db::Meta {
            &mut self.0
        }

        fn encode(&self, _out: &mut Vec<sofie_core::SqlValue>) {}

        fn decode(meta: sofie_db::Meta, _row: &mut dyn sofie_core::ColumnReader) -> Result<Self> {
            Ok(Self(meta))
        }
    }

    #[cfg(feature = "groups")]
    struct Filed(sofie_db::Meta);

    #[cfg(feature = "groups")]
    impl Aggregate for Filed {
        const NAME: &'static str = "filed";
        const MODULE: &'static str = "shop";
        const FIELDS: &'static [sofie_db::Field] = &[sofie_db::Field::new(
            "folder",
            <sofie_core::Uuid as sofie_core::Cast>::COLUMNS,
            false,
        )];
        const OWNER: Option<&'static str> = Some("folder");

        fn meta(&self) -> &sofie_db::Meta {
            &self.0
        }

        fn meta_mut(&mut self) -> &mut sofie_db::Meta {
            &mut self.0
        }

        fn encode(&self, _out: &mut Vec<sofie_core::SqlValue>) {}

        fn decode(meta: sofie_db::Meta, _row: &mut dyn sofie_core::ColumnReader) -> Result<Self> {
            Ok(Self(meta))
        }

        fn owned_by(owner: sofie_core::Uuid) -> Option<Condition<Self>> {
            sofie_db::owned_through::<Self, Folder>(sofie_db::Column::new("folder"), owner)
        }

        fn children(&self) -> Vec<&dyn sofie_db::ChildRows> {
            Vec::new()
        }

        fn children_mut(&mut self) -> Vec<&mut dyn sofie_db::ChildRows> {
            Vec::new()
        }
    }

    #[cfg(feature = "groups")]
    #[tokio::test]
    async fn a_list_under_a_parent_beyond_reach_is_not_found() {
        let db = sofie_db::Db::new(sofie_testing::FakeDriver::default());
        let owner = sofie_core::Uuid::now_v7().unwrap();
        let read = Read::within(&db, sofie_db::Reach::OwnedBy(owner));
        let mut listing = asked::<Filed, ()>("").unwrap();
        let folder = sofie_core::Uuid::now_v7().unwrap().to_string();
        listing.parents = vec![("folder".to_owned(), folder)];

        let listed = listing.page(&read).await;

        assert!(matches!(listed, Err(Error::NotFound)), "{:?}", listed.err());
    }
}
