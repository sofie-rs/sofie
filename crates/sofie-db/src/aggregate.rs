use std::any::TypeId;
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::Range;

use serde::de::DeserializeOwned;
use serde_json::Value;
use sofie_core::{
    Cast, ColumnReader, ColumnSpec, Error, FieldError, Instant, Result, RowReader, SqlType,
    SqlValue, Uuid,
};

use crate::condition::Kind;
use crate::sql::column_name;
use crate::type_cache::TypeCache;
use crate::{Behavior, Column, Condition, Dialect, Event, Id, Selected, Versioned};

#[derive(Clone, Debug)]
pub struct Meta {
    id: Uuid,
    /// `version` first; empty until first saved.
    pub(crate) stored: Vec<SqlValue>,
    pub(crate) snapshot: Option<Snapshot>,
    events: Vec<RaisedEvent>,
}

#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    pub(crate) stored: Vec<SqlValue>,
    pub(crate) fields: Vec<SqlValue>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RaisedEvent {
    pub(crate) name: &'static str,
    pub(crate) payload: Value,
}

impl RaisedEvent {
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    #[must_use]
    pub fn payload(&self) -> &Value {
        &self.payload
    }
}

impl Meta {
    pub fn new() -> Result<Self> {
        Ok(Self {
            id: Uuid::now_v7()?,
            stored: Vec::new(),
            snapshot: None,
            events: Vec::new(),
        })
    }

    #[must_use]
    pub fn id(&self) -> Uuid {
        self.id
    }

    #[must_use]
    pub fn version(&self) -> i64 {
        crate::behavior::version(self.stored.first())
    }

    /// Refuses text holding NUL, which the database cannot store.
    pub fn raise<E: Event>(&mut self, event: E) -> Result<()> {
        let payload = serde_json::to_value(event)
            .map_err(|error| Error::internal(format!("serializing event `{}`", E::NAME), error))?;
        if holds_nul(&payload) {
            return Err(Error::BadRequest(format!(
                "event `{}` holds a NUL character, which cannot be stored",
                E::NAME
            )));
        }
        if crate::event::is_pruned(&payload) {
            return Err(Error::internal_message(format!(
                "event `{}` sets `{}`, which only a pruned record's change event carries",
                E::NAME,
                crate::event::PRUNED
            )));
        }
        self.events.push(RaisedEvent {
            name: E::NAME,
            payload,
        });
        Ok(())
    }

    #[must_use]
    pub fn events(&self) -> &[RaisedEvent] {
        &self.events
    }

    pub(crate) fn written(&mut self, written: Option<Snapshot>) {
        self.events.clear();
        if let Some(written) = written {
            self.stored.clone_from(&written.stored);
            self.snapshot = Some(written);
        }
    }
}

fn holds_nul(value: &Value) -> bool {
    match value {
        Value::String(text) => text.contains('\0'),
        Value::Array(values) => values.iter().any(holds_nul),
        Value::Object(fields) => fields
            .iter()
            .any(|(name, value)| name.contains('\0') || holds_nul(value)),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

#[derive(Debug)]
pub struct Field {
    pub name: &'static str,
    pub columns: &'static [ColumnSpec],
    pub nullable: bool,
    pub parse_text: fn(&str) -> Option<Result<SqlValue, FieldError>>,
    /// Whether the server can read the stored value back, so an edit left blank keeps it.
    pub handed_back: fn() -> bool,
}

impl Field {
    #[must_use]
    pub const fn new(name: &'static str, columns: &'static [ColumnSpec], nullable: bool) -> Self {
        Self {
            name,
            columns,
            nullable,
            parse_text: |_| None,
            handed_back: || true,
        }
    }

    #[must_use]
    pub const fn handed_back_by(self, handed_back: fn() -> bool) -> Self {
        Self {
            handed_back,
            ..self
        }
    }

    /// A list filter reads the text by the column's type only when this gives `None`, no reader.
    #[must_use]
    pub const fn parsed_by(
        self,
        parse_text: fn(&str) -> Option<Result<SqlValue, FieldError>>,
    ) -> Self {
        Self { parse_text, ..self }
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn stores(fields: &[Self], column: &str) -> bool {
        let mut index = 0;
        while index < fields.len() {
            let field = &fields[index];
            let mut spec = 0;
            while spec < field.columns.len() {
                if sofie_core::same_column(field.name, field.columns[spec].suffix, column, "") {
                    return true;
                }
                spec += 1;
            }
            index += 1;
        }
        false
    }
}

/// Autoref specialization: `(&&TextOf::<T>(PhantomData)).read(text)` reads JSON text (a string,
/// else a number or bool) as `T` does when `T` deserializes, and gives nothing otherwise.
#[doc(hidden)]
pub struct TextOf<T>(pub PhantomData<T>);

#[doc(hidden)]
pub trait ReadsText {
    fn read(&self, text: &str) -> Option<Result<SqlValue, FieldError>>;
}

impl<T: DeserializeOwned + Cast> ReadsText for &TextOf<T> {
    fn read(&self, text: &str) -> Option<Result<SqlValue, FieldError>> {
        Some(read_text::<T>(text))
    }
}

fn read_text<T: DeserializeOwned + Cast>(text: &str) -> Result<SqlValue, FieldError> {
    let value: T = serde_json::from_value(Value::String(text.to_owned())).or_else(|refusal| {
        match serde_json::from_str::<Value>(text) {
            Ok(scalar @ (Value::Number(_) | Value::Bool(_))) => {
                serde_json::from_value(scalar).map_err(|_| refusal_of::<T>(text, &refusal))
            }
            _ => Err(refusal_of::<T>(text, &refusal)),
        }
    })?;
    let mut encoded = Vec::with_capacity(1);
    value.encode(&mut encoded);
    match (encoded.pop(), encoded.is_empty()) {
        (Some(single), true) => Ok(single),
        _ => Err(FieldError::invalid("", "it is not a value of one column")),
    }
}

/// serde words a kind mismatch "invalid type: …, expected i64", naming Rust types, and an id
/// refuses in Uuid's technical words; a type's own refusal, such as a bound or an instant's
/// repeated wall clock, is kept as the type wrote it.
fn refusal_of<T: Cast>(text: &str, refusal: &serde_json::Error) -> FieldError {
    let message = refusal.to_string();
    match T::COLUMNS {
        [column] if column.ty == SqlType::Uuid || message.starts_with("invalid type: ") => {
            expected(column.ty)
        }
        [column] if column.ty == SqlType::Timestamp => instant_refusal(text, message),
        _ => FieldError::invalid("", message),
    }
}

/// serde carries only a refusal's words, so an instant's is read again for its key and arguments.
fn instant_refusal(text: &str, message: String) -> FieldError {
    match Instant::parse(text) {
        Err(Error::Invalid(mut errors)) if errors.len() == 1 && errors[0].message == message => {
            errors.remove(0)
        }
        _ => FieldError::invalid("", message),
    }
}

pub(crate) fn expected(ty: SqlType) -> FieldError {
    let (key, message) = match ty {
        SqlType::Int => ("validation.kind.whole_number", "must be a whole number"),
        SqlType::Float => ("validation.kind.number", "must be a number"),
        SqlType::Bool => ("validation.kind.true_or_false", "must be true or false"),
        SqlType::Uuid => ("validation.kind.id", "must be an id"),
        SqlType::Date => ("validation.kind.date", "must be a date"),
        SqlType::DateTime | SqlType::Timestamp => {
            ("validation.kind.date_time", "must be a date and time")
        }
        SqlType::Text | SqlType::Bytes => ("validation.kind.text", "is not valid"),
    };
    FieldError::new("", key, message)
}

#[doc(hidden)]
pub trait ReadsNoText {
    fn read(&self, text: &str) -> Option<Result<SqlValue, FieldError>>;
}

impl<T> ReadsNoText for TextOf<T> {
    fn read(&self, _text: &str) -> Option<Result<SqlValue, FieldError>> {
        None
    }
}

/// Autoref specialization, as [`TextOf`]: whether a type serializes, asked without a value.
#[doc(hidden)]
pub struct JsonProbe<T>(pub PhantomData<T>);

#[doc(hidden)]
pub trait Serializes {
    fn serializes(&self) -> bool;
}

impl<T: serde::Serialize> Serializes for &JsonProbe<T> {
    fn serializes(&self) -> bool {
        true
    }
}

#[doc(hidden)]
pub trait SerializesNot {
    fn serializes(&self) -> bool;
}

impl<T> SerializesNot for JsonProbe<T> {
    fn serializes(&self) -> bool {
        false
    }
}

/// Autoref specialization, as [`TextOf`]: a field's JSON when its type serializes.
#[doc(hidden)]
pub struct JsonOf<'a, T>(pub &'a T);

#[doc(hidden)]
pub trait WritesJson {
    fn json(&self) -> Option<Value>;
}

impl<T: serde::Serialize> WritesJson for &JsonOf<'_, T> {
    fn json(&self) -> Option<Value> {
        serde_json::to_value(self.0).ok()
    }
}

#[doc(hidden)]
pub trait WritesNoJson {
    fn json(&self) -> Option<Value>;
}

impl<T> WritesNoJson for JsonOf<'_, T> {
    fn json(&self) -> Option<Value> {
        None
    }
}

pub trait Source: Sized + Send + 'static {
    type Base: Aggregate;

    fn columns() -> Vec<Selected>;

    fn column_types() -> Vec<Option<SqlType>>;

    fn decode(row: &mut dyn ColumnReader) -> Result<Self>;
}

pub trait Aggregate: Sized + Send + Sync + 'static {
    const NAME: &'static str;
    const MODULE: &'static str;
    const FIELDS: &'static [Field];
    const OWNER: Option<&'static str> = None;
    const BEHAVIORS: &'static [&'static dyn Behavior] = &[];
    const CHANGED: Option<&'static str> = None;
    const PRUNED: Option<&'static str> = None;
    /// Fields held unique by the database, answered as their own field errors.
    const UNIQUE: &'static [&'static str] = &[];
    /// `#[sensitive]` fields, which a list can neither filter nor sort by.
    const SENSITIVE: &'static [&'static str] = &[];
    const ID: Column<Self, Id<Self>> = Column::new("id");

    fn meta(&self) -> &Meta;
    fn meta_mut(&mut self) -> &mut Meta;
    fn encode(&self, out: &mut Vec<SqlValue>);

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self>;

    fn id(&self) -> Id<Self> {
        Id::from_uuid(self.meta().id)
    }

    /// Fields of a sensitive type, left out of its JSON; read on the server so a blank edit keeps them.
    #[doc(hidden)]
    fn sensitive_json(&self) -> serde_json::Map<String, Value> {
        serde_json::Map::new()
    }

    #[cfg(feature = "children")]
    const CHILDREN: &'static [&'static crate::ChildTable] = &[];

    fn owner(&self) -> Option<Uuid> {
        None
    }

    #[must_use]
    fn owned_by(owner: Uuid) -> Option<Condition<Self>> {
        Some(Condition::new(uuid_equals(Self::OWNER?, owner)))
    }

    #[cfg(feature = "children")]
    fn children(&self) -> Vec<&dyn crate::ChildRows> {
        Vec::new()
    }

    #[cfg(feature = "children")]
    fn children_mut(&mut self) -> Vec<&mut dyn crate::ChildRows> {
        Vec::new()
    }

    #[cfg(feature = "contracts")]
    fn exposed_fields(&self) -> Option<Result<serde_json::Map<String, Value>>> {
        None
    }

    fn refuse_unlisted_moves(&self, loaded: &mut dyn ColumnReader) -> Result<()> {
        let _ = loaded;
        Ok(())
    }
}

/// The column holding the owner itself, unless the owner is reached through a parent.
#[cfg(feature = "search-engine")]
pub(crate) fn direct_owner<A: Aggregate>() -> Option<&'static str> {
    let column = A::OWNER?;
    let probe = Uuid::from_u128(0);
    (A::owned_by(probe)?.kind == uuid_equals(column, probe)).then_some(column)
}

/// How many child kinds an aggregate holds, each one JSON column after its own fields; for
/// sofie-testing, which rebuilds a record from its encoded fields.
#[doc(hidden)]
#[must_use]
pub fn child_tables<A: Aggregate>() -> usize {
    #[cfg(feature = "children")]
    return A::CHILDREN.len();
    #[cfg(not(feature = "children"))]
    0
}

/// How many of an aggregate's fields are sensitive, by attribute or by type; for the derive.
#[doc(hidden)]
#[must_use]
pub const fn sensitive_count(fields: &[(&str, bool)]) -> usize {
    let (mut index, mut count) = (0, 0);
    while index < fields.len() {
        if fields[index].1 {
            count += 1;
        }
        index += 1;
    }
    count
}

/// The names of the sensitive fields, so `Aggregate::SENSITIVE` stays a constant.
#[doc(hidden)]
#[must_use]
pub const fn sensitive_names<const N: usize>(fields: &[(&'static str, bool)]) -> [&'static str; N] {
    let mut names = [""; N];
    let (mut index, mut kept) = (0, 0);
    while index < fields.len() {
        if fields[index].1 {
            names[kept] = fields[index].0;
            kept += 1;
        }
        index += 1;
    }
    names
}

#[cfg(test)]
mod sensitive_tests {
    use super::*;

    #[test]
    fn the_sensitive_names_are_the_marked_fields_in_order() {
        const FIELDS: &[(&str, bool)] = &[("name", false), ("iban", true), ("pin", true)];
        const NAMES: [&str; sensitive_count(FIELDS)] = sensitive_names(FIELDS);

        assert_eq!(NAMES, ["iban", "pin"]);
    }
}

pub(crate) fn uuid_equals(column: &str, owner: Uuid) -> Kind {
    Kind::Compare {
        columns: vec![column.to_owned()],
        operator: "=",
        values: vec![SqlValue::Uuid(owner)],
    }
}

pub(crate) struct Table {
    pub(crate) schema: &'static str,
    pub(crate) name: &'static str,
    pub(crate) behaviors: Vec<(&'static dyn Behavior, Range<usize>)>,
    pub(crate) stored: Vec<String>,
    pub(crate) columns: Vec<String>,
    /// Of the whole selected row: id, `stored`, `columns`, then child rows.
    types: Vec<Option<SqlType>>,
    #[cfg(any(feature = "flags", feature = "search-engine"))]
    pub(crate) owner: Option<&'static str>,
}

impl Table {
    pub(crate) fn qualified(&self, dialect: &dyn Dialect) -> String {
        dialect.qualified(self.schema, self.name)
    }

    pub(crate) fn stored_index(&self, column: &str) -> usize {
        self.stored
            .iter()
            .position(|stored| stored == column)
            .unwrap_or_else(|| panic!("`{}` has no behavior column `{column}`", self.name))
    }

    pub(crate) fn row_columns(&self) -> impl Iterator<Item = &str> {
        std::iter::once("id").chain(self.stored.iter().chain(&self.columns).map(String::as_str))
    }

    pub(crate) fn hiding(&self) -> impl Iterator<Item = &'static str> {
        self.behaviors
            .iter()
            .filter_map(|(behavior, _)| behavior.hides())
    }
}

/// Read per query and per decoded row, so each thread keeps its own copy of the lookup and
/// takes the shared lock only the first time it meets a type.
pub(crate) fn table<A: Aggregate>() -> &'static Table {
    static TABLES: TypeCache<TypeId, &'static Table> = TypeCache::new();
    thread_local! {
        static SEEN: RefCell<HashMap<TypeId, &'static Table>> = RefCell::default();
    }
    let key = TypeId::of::<A>();
    SEEN.with_borrow_mut(|seen| {
        *seen.entry(key).or_insert_with(|| {
            TABLES.get_or_insert_with(key, || Box::leak(Box::new(describe::<A>())))
        })
    })
}

fn describe<A: Aggregate>() -> Table {
    let mut behaviors = Vec::new();
    let mut stored = Vec::new();
    let mut types = vec![Some(SqlType::Uuid)];
    for behavior in std::iter::once(&Versioned as &dyn Behavior).chain(A::BEHAVIORS.iter().copied())
    {
        let start = stored.len();
        stored.extend(field_columns(behavior.columns()));
        types.extend(field_types(behavior.columns()));
        behaviors.push((behavior, start..stored.len()));
    }
    types.extend(field_types(A::FIELDS));
    #[cfg(feature = "children")]
    types.extend(A::CHILDREN.iter().map(|_| None));
    let table = Table {
        schema: A::MODULE,
        name: A::NAME,
        behaviors,
        stored,
        columns: field_columns(A::FIELDS),
        types,
        #[cfg(any(feature = "flags", feature = "search-engine"))]
        owner: A::OWNER,
    };
    refuse_repeated_columns(&table, A::FIELDS);
    #[cfg(feature = "flags")]
    refuse_broadcast_defaults(&table);
    table
}

#[cfg(feature = "flags")]
fn refuse_broadcast_defaults(table: &Table) {
    let mut behaviors = table.behaviors.iter().map(|(behavior, _)| *behavior);
    let default = std::any::type_name::<crate::Default>();
    let defaults = behaviors.clone().any(|behavior| behavior.name() == default);
    assert!(
        !(defaults && behaviors.any(Behavior::broadcasts)),
        "`{}` uses both `Default` and `Broadcasts`: the records a new default clears would \
         change without a change event",
        table.name
    );
}

/// A column stored twice fails only at the first write, as Postgres's 42701. The derive refuses
/// the columns every aggregate has, but an opt-in behavior's are known only through the dyn
/// `Behavior::columns`, so the check runs here, when the type is first used.
fn refuse_repeated_columns(table: &Table, fields: &[Field]) {
    let by_behaviors = table.behaviors.iter().flat_map(|(behavior, range)| {
        table.stored[range.clone()]
            .iter()
            .map(|column| (column, format!("behavior `{}`", behavior.name())))
    });
    let by_fields = fields
        .iter()
        .flat_map(|field| {
            field
                .columns
                .iter()
                .map(|_| format!("field `{}`", field.name))
        })
        .zip(&table.columns)
        .map(|(owner, column)| (column, owner));
    let mut seen = vec![("id", "the record id".to_owned())];
    for (column, owner) in by_behaviors.chain(by_fields) {
        if let Some((_, earlier)) = seen.iter().find(|(name, _)| name == column) {
            panic!(
                "`{}` has two columns named `{column}`: {owner} repeats {earlier}; a field \
                 cannot take the name of a behavior's column",
                table.name
            );
        }
        seen.push((column, owner));
    }
}

fn field_types(fields: &[Field]) -> impl Iterator<Item = Option<SqlType>> {
    fields
        .iter()
        .flat_map(|field| field.columns.iter().map(|column| Some(column.ty)))
}

pub(crate) fn field_columns(fields: &[Field]) -> Vec<String> {
    fields
        .iter()
        .flat_map(|field| {
            field
                .columns
                .iter()
                .map(|column| column_name(field.name, column))
        })
        .collect()
}

impl<A: Aggregate> Source for A {
    type Base = A;

    fn columns() -> Vec<Selected> {
        let table = table::<A>();
        let columns = table.row_columns().map(Selected::column);
        #[cfg(feature = "children")]
        let columns = columns.chain(A::CHILDREN.iter().map(|child| child.selected()));
        columns.collect()
    }

    fn column_types() -> Vec<Option<SqlType>> {
        table::<A>().types.clone()
    }

    /// Its stored columns are kept as values, as a save compares them to tell what changed.
    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        let table = table::<A>();
        let id = Uuid::decode(row)?;
        let stored = values(row, table.stored.len())?;
        let fields = values(row, table.columns.len())?;
        let meta = Meta {
            id,
            stored: stored.clone(),
            snapshot: None,
            events: Vec::new(),
        };
        let mut rest = FieldsThenRow {
            fields: RowReader::new(&fields),
            row,
        };
        let mut aggregate = <A as Aggregate>::decode(meta, &mut rest)?;
        aggregate.meta_mut().snapshot = Some(Snapshot { stored, fields });
        Ok(aggregate)
    }
}

fn values(row: &mut dyn ColumnReader, count: usize) -> Result<Vec<SqlValue>> {
    (0..count).map(|_| row.value()).collect()
}

/// Child rows are read from the row itself, so their JSON is not copied into the snapshot.
struct FieldsThenRow<'a> {
    fields: RowReader<'a>,
    row: &'a mut dyn ColumnReader,
}

impl FieldsThenRow<'_> {
    fn next(&mut self) -> &mut dyn ColumnReader {
        if self.fields.remaining() > 0 {
            return &mut self.fields;
        }
        &mut *self.row
    }
}

macro_rules! from_next {
    ($($method:ident -> $type:ty),*) => {
        $(fn $method(&mut self) -> Result<$type> {
            self.next().$method()
        })*
    };
}

impl ColumnReader for FieldsThenRow<'_> {
    fn remaining(&self) -> usize {
        self.fields.remaining() + self.row.remaining()
    }

    fn is_null_at(&self, ahead: usize) -> Result<bool> {
        let fields = self.fields.remaining();
        if ahead < fields {
            return self.fields.is_null_at(ahead);
        }
        self.row.is_null_at(ahead - fields)
    }

    from_next!(
        text -> &str, bytes -> &[u8], skip -> (), bool -> bool, int -> i64, float -> f64, uuid -> Uuid,
        timestamp -> sofie_core::Timestamp, date -> sofie_core::Date,
        date_time -> sofie_core::DateTime,
        json -> Value, value -> SqlValue
    );
}

#[must_use]
pub fn read_columns<A: Aggregate>(fields: &[&str]) -> Vec<Selected> {
    let table = table::<A>();
    fields
        .iter()
        .flat_map(|name| {
            if *name == "id" || table.stored.iter().any(|column| column == name) {
                return vec![Selected::column(*name)];
            }
            let field = A::FIELDS
                .iter()
                .find(|field| field.name == *name)
                .unwrap_or_else(|| panic!("`{name}` is not a field of `{}`", A::NAME));
            field_columns(std::slice::from_ref(field))
                .into_iter()
                .map(Selected::column)
                .collect()
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{MessageSql, Migrations, SoftDelete, Timestamps, Uses};

    pub(crate) struct Note {
        pub(crate) meta: Meta,
        pub(crate) title: String,
    }

    impl Aggregate for Note {
        const NAME: &'static str = "note";
        const MODULE: &'static str = "notes";
        const FIELDS: &'static [Field] = &[Field::new("title", String::COLUMNS, false)];
        const OWNER: Option<&'static str> = Some("id");
        const BEHAVIORS: &'static [&'static dyn Behavior] = &[&Timestamps, &SoftDelete];

        fn meta(&self) -> &Meta {
            &self.meta
        }

        fn meta_mut(&mut self) -> &mut Meta {
            &mut self.meta
        }

        fn owner(&self) -> Option<Uuid> {
            Some(self.meta.id())
        }

        fn encode(&self, out: &mut Vec<SqlValue>) {
            self.title.encode(out);
        }

        fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
            Ok(Self {
                meta,
                title: String::decode(row)?,
            })
        }
    }

    pub(crate) struct Numbered(pub(crate) &'static MessageSql);

    pub(crate) const NUMBERED: Numbered = Numbered(&MESSAGES);

    pub(crate) const MESSAGES: MessageSql = MessageSql {
        insert: "INSERT INTO sofie.messages",
        fan_out: "",
        fan_out_events: "",
        claim: "",
        renew: "",
        complete: "",
        retry: "",
        bury: "",
        to_job: "",
        add_schedule: "",
        fire_schedule: "",
        notify: None,
    };

    impl Dialect for Numbered {
        fn push_placeholder(&self, sql: &mut String, index: usize) {
            sql.push('$');
            sql.push_str(&index.to_string());
        }

        fn migrations_setup(&self) -> &'static str {
            unimplemented!()
        }

        fn migrations_lock(&self) -> &'static str {
            unimplemented!()
        }

        fn enter_schema(&self, _schema: &str) -> String {
            unimplemented!()
        }

        fn framework_migrations(&self) -> Migrations {
            unimplemented!()
        }

        fn messages(&self) -> &'static MessageSql {
            self.0
        }
    }

    impl Uses<Timestamps> for Note {}
    impl Uses<SoftDelete> for Note {}

    struct Clash {
        meta: Meta,
    }

    impl Aggregate for Clash {
        const NAME: &'static str = "clash";
        const MODULE: &'static str = "notes";
        const FIELDS: &'static [Field] = &[Field::new("deleted_at", String::COLUMNS, false)];
        const BEHAVIORS: &'static [&'static dyn Behavior] = &[&Timestamps, &SoftDelete];

        fn meta(&self) -> &Meta {
            &self.meta
        }

        fn meta_mut(&mut self) -> &mut Meta {
            &mut self.meta
        }

        fn encode(&self, _out: &mut Vec<SqlValue>) {}

        fn decode(meta: Meta, _row: &mut dyn ColumnReader) -> Result<Self> {
            Ok(Self { meta })
        }
    }

    fn names(selection: Vec<Selected>) -> Vec<String> {
        selection
            .into_iter()
            .map(|selected| selected.render(table::<Note>(), &NUMBERED, None, &mut Vec::new()))
            .map(|quoted| quoted.trim_matches('"').to_owned())
            .collect()
    }

    #[test]
    fn an_aggregate_selects_its_id_behavior_columns_then_fields() {
        assert_eq!(
            names(<Note as Source>::columns()),
            [
                "id",
                "version",
                "created_at",
                "updated_at",
                "deleted_at",
                "title"
            ]
        );
    }

    #[test]
    fn an_aggregate_expects_the_type_of_each_column_it_selects() {
        let timestamp = Some(SqlType::Timestamp);

        assert_eq!(
            <Note as Source>::column_types(),
            [
                Some(SqlType::Uuid),
                Some(SqlType::Int),
                timestamp,
                timestamp,
                timestamp,
                Some(SqlType::Text)
            ]
        );
    }

    #[test]
    fn every_thread_reads_the_one_description_of_a_type() {
        let address = || std::ptr::from_ref(table::<Note>()) as usize;

        let elsewhere = std::thread::spawn(address).join().unwrap();

        assert_eq!(address(), elsewhere);
        assert_eq!(address(), address());
    }

    #[test]
    fn what_follows_the_fields_is_read_from_the_row_itself() {
        let fields = [SqlValue::Int(1)];
        let children = [SqlValue::Text("child".to_owned())];
        let mut row = RowReader::new(&children);
        let mut reader = FieldsThenRow {
            fields: RowReader::new(&fields),
            row: &mut row,
        };

        assert_eq!(reader.remaining(), 2);
        assert_eq!(reader.int().unwrap(), 1);
        assert!(!reader.is_null().unwrap());
        assert_eq!(reader.text().unwrap(), "child");
        assert!(reader.skip().is_err());
    }

    #[test]
    fn a_read_model_selects_fields_and_behavior_columns_of_its_base() {
        assert_eq!(
            names(read_columns::<Note>(&["id", "title", "created_at"])),
            ["id", "title", "created_at"]
        );
    }

    #[test]
    #[should_panic(expected = "`colour` is not a field of `note`")]
    fn a_read_model_cannot_select_what_its_base_lacks() {
        let _ = read_columns::<Note>(&["colour"]);
    }

    struct Renamed {
        title: String,
    }

    impl crate::Input for Renamed {
        fn from_json(_value: Value) -> Result<Self> {
            unimplemented!("only raised")
        }
    }

    impl serde::Serialize for Renamed {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.collect_map([("titles", [&self.title])])
        }
    }

    impl Event for Renamed {
        const NAME: &'static str = "notes.Renamed";
    }

    struct Masked;

    impl crate::Input for Masked {
        fn from_json(_: Value) -> Result<Self> {
            Ok(Self)
        }
    }

    impl serde::Serialize for Masked {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.collect_map([("sofie.pruned", true)])
        }
    }

    impl Event for Masked {
        const NAME: &'static str = "notes.Masked";
    }

    #[test]
    fn an_app_event_cannot_pass_as_a_pruned_record() {
        let mut meta = Meta::new().unwrap();

        let refused = meta.raise(Masked);

        assert!(
            matches!(&refused, Err(Error::Internal { context, .. }) if context.contains("notes.Masked")),
            "{refused:?}"
        );
    }

    #[test]
    fn an_event_holding_nul_is_refused_when_raised() {
        let mut meta = Meta::new().unwrap();

        let refused = meta.raise(Renamed {
            title: "a\0b".to_owned(),
        });

        assert!(
            matches!(&refused, Err(Error::BadRequest(message)) if message.contains("notes.Renamed")),
            "{refused:?}"
        );
        assert_eq!(meta.events(), []);
        meta.raise(Renamed {
            title: "ab".to_owned(),
        })
        .unwrap();
        assert_eq!(meta.events().len(), 1);
    }

    #[test]
    #[should_panic(
        expected = "`clash` has two columns named `deleted_at`: field `deleted_at` repeats \
                    behavior `sofie_db::soft_delete::SoftDelete`"
    )]
    fn a_field_cannot_take_an_opt_in_behaviors_column() {
        let _ = <Clash as Source>::columns();
    }
}
