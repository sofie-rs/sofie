use serde_json::{Map, Value};
use sofie_core::{Error, Result};
use sofie_db::{
    Aggregate, Context, Create, Id, Input, Outcome, Plan, Read, ReadOnly, Reply, ReplyData,
    Resolve, Stamp, delete_softly,
};
use std::marker::PhantomData;

use sofie_values::{Control, FormField, FormFields};

use super::{Look, ShownField};
use crate::gate::optional::listing::Listing;
use crate::{Area, RequestContext};

/// Holds the path parameters, so its links name the same parents.
pub(super) struct Shown(Map<String, Value>);

impl Outcome for Shown {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(Value::Object(self.0)))
    }
}

impl ReadOnly for Shown {}

/// Shown, never returned, so a GET may load the aggregate itself.
pub(crate) struct Record<R>(R);

impl<A: Area, M, R: Aggregate> Resolve<RequestContext<A, M>> for Record<R> {
    async fn resolve(context: &mut RequestContext<A, M>) -> Result<Self> {
        sofie_db::bind(context).await.map(Self)
    }
}

pub(crate) struct Parameters(Map<String, Value>);

impl<A: Area, M> Resolve<RequestContext<A, M>> for Parameters {
    fn resolve(context: &mut RequestContext<A, M>) -> impl Future<Output = Result<Self>> + Send {
        let parameters = context
            .matched
            .parameters
            .iter()
            .map(|(name, value)| (name.clone(), Value::String(value.clone())))
            .collect();
        std::future::ready(Ok(Self(parameters)))
    }
}

/// Parents come from the path, not the post, so a record cannot be moved under another parent.
pub(super) struct Posted<I>(I);

impl<A: Area, I: Input + FormFields + Send> Resolve<RequestContext<A>> for Posted<I> {
    fn resolve(context: &mut RequestContext<A>) -> impl Future<Output = Result<Self>> + Send {
        let posted = context.input().and_then(|mut input| {
            bind_parents(&mut input, &context.matched.parameters, I::FIELDS);
            I::from_json(input).map(Self)
        });
        std::future::ready(posted)
    }
}

/// Read only once the record is loaded, as a blank sensitive field keeps what it stores.
pub(super) struct Edited<I>(Value, PhantomData<fn() -> I>);

impl<A: Area, I: FormFields> Resolve<RequestContext<A>> for Edited<I> {
    fn resolve(context: &mut RequestContext<A>) -> impl Future<Output = Result<Self>> + Send {
        let edited = context.input().map(|mut input| {
            bind_parents(&mut input, &context.matched.parameters, I::FIELDS);
            Self(input, PhantomData)
        });
        std::future::ready(edited)
    }
}

fn bind_parents(input: &mut Value, parameters: &[(String, String)], fields: &[FormField]) {
    let Value::Object(posted) = input else {
        return;
    };
    for (name, value) in parameters {
        if posted.contains_key(name) || fields.iter().any(|field| field.name == name) {
            posted.insert(name.clone(), Value::String(value.clone()));
        }
    }
}

pub trait OwnerOf<A: Area>: Sized + Send {
    /// `NotFound` when the area's guard let nobody in to own the record.
    fn of(context: &RequestContext<A>) -> Result<Self>;
}

impl<A: Area> OwnerOf<A> for () {
    fn of(_context: &RequestContext<A>) -> Result<Self> {
        Ok(())
    }
}

impl<A: Area<Principal = T>, T: 'static> OwnerOf<A> for Id<T> {
    fn of(context: &RequestContext<A>) -> Result<Self> {
        context
            .session()
            .principal_in(A::NAME)
            .map(Id::from_uuid)
            .ok_or(Error::NotFound)
    }
}

pub(super) struct Owner<O>(O);

impl<A: Area, O: OwnerOf<A>> Resolve<RequestContext<A>> for Owner<O> {
    fn resolve(context: &mut RequestContext<A>) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(O::of(context).map(Self))
    }
}

/// A duplicate on a `<table>_<field>` unique index is answered as that field's error.
pub(super) struct Saved<R> {
    record: R,
    unique: &'static [(&'static str, &'static str)],
}

const TAKEN: &str = "is already taken";

impl<R: Aggregate> Outcome for Saved<R> {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        for (index, field) in self.unique {
            plan.unique(index, field, TAKEN);
        }
        self.record.plan(plan)
    }

    fn committed(&mut self, stamp: Stamp) {
        self.record.committed(stamp);
    }

    fn reply(self) -> Result<Reply> {
        self.record.reply()
    }
}

pub(super) async fn list<R: Aggregate + ReplyData>(
    read: Read,
    listing: Listing<R>,
    parameters: Parameters,
) -> Result<Shown> {
    let mut shown = parameters.0;
    shown.insert("list".to_owned(), listing.page(&read).await?.to_json()?);
    Ok(Shown(shown))
}

pub(super) async fn show<R: Aggregate + ReplyData>(
    Record(record): Record<R>,
    read: Read,
    parameters: Parameters,
    fields: &'static [ShownField],
) -> Result<Shown> {
    let mut shown = parameters.0;
    let value = record
        .to_json()
        .map_err(|error| Error::internal(format!("serializing a {}", R::NAME), error))?;
    let display = display(&value, fields);
    if !display.is_empty() {
        shown.insert("display".to_owned(), Value::Object(display));
    }
    shown.insert("record".to_owned(), value);
    #[cfg(feature = "history")]
    shown.insert(
        "history".to_owned(),
        super::history::changes(&read, &record).await?,
    );
    let _ = read;
    Ok(Shown(shown))
}

/// What a template cannot work out itself: a percentage from its basis points, and a value of
/// parts no input names from the text of each part.
fn display(record: &Value, fields: &[ShownField]) -> Map<String, Value> {
    fields
        .iter()
        .filter_map(|field| {
            let value = record.get(field.name)?;
            let text = match field.look {
                Look::Percentage => {
                    format!("{} %", sofie_values::percent_text(value.as_i64()?))
                }
                Look::Joined => joined_text(value),
                _ => return None,
            };
            Some((field.name.to_owned(), Value::String(text)))
        })
        .collect()
}

fn joined_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Array(values) => values.iter().map(joined_text).collect::<Vec<_>>().join(" "),
        Value::Object(parts) => parts
            .values()
            .map(joined_text)
            .collect::<Vec<_>>()
            .join(" "),
        Value::Bool(_) | Value::Number(_) => value.to_string(),
    }
}

pub(super) async fn blank(parameters: Parameters) -> Result<Shown> {
    Ok(Shown(parameters.0))
}

pub(super) async fn create<R: Aggregate + Create>(
    posted: Posted<R::Input>,
    owner: Owner<R::Owner>,
    unique: &'static [(&'static str, &'static str)],
) -> Result<Saved<R>> {
    let record = R::create(posted.0, owner.0)?;
    Ok(Saved { record, unique })
}

pub(super) async fn update<R: Aggregate + ReplyData + sofie_db::Update>(
    mut record: R,
    edited: Edited<R::Input>,
    unique: &'static [(&'static str, &'static str)],
) -> Result<Saved<R>>
where
    R::Input: FormFields,
{
    let Edited(mut posted, _) = edited;
    let mut stored = record
        .to_json()
        .map_err(|error| Error::internal(format!("serializing a {}", R::NAME), error))?;
    if let Value::Object(fields) = &mut stored {
        fields.extend(record.sensitive_json());
    }
    keep_blank_secrets(&mut posted, &stored, R::Input::FIELDS);
    record.update(R::Input::from_json(posted)?)?;
    Ok(Saved { record, unique })
}

/// The edit form never shows a sensitive field, so leaving it blank keeps it. One the record's
/// JSON leaves out cannot be kept, and stays required.
fn keep_blank_secrets(posted: &mut Value, stored: &Value, fields: &[FormField]) {
    let (Value::Object(posted), Value::Object(stored)) = (posted, stored) else {
        return;
    };
    for field in fields.iter().filter(|field| is_secret(field)) {
        let blank = posted
            .get(field.name)
            .is_none_or(|value| value.is_null() || value.as_str() == Some(""));
        if let (true, Some(kept)) = (blank, stored.get(field.name)) {
            posted.insert(field.name.to_owned(), kept.clone());
        }
    }
}

pub(super) fn is_secret(field: &FormField) -> bool {
    field.input.control == Control::Password
}

pub(super) async fn delete<R: Aggregate>(mut record: R) -> Result<R> {
    delete_softly(&mut record)?;
    Ok(record)
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sofie_values::Control;

    use super::*;

    #[test]
    fn a_post_is_bound_to_the_parents_its_path_names_even_when_it_leaves_them_out() {
        const ROLE: FormField = FormField {
            name: "role",
            input: sofie_values::Input::new(Control::Input),
            required: false,
            constraints: &[],
        };
        let parameters = [
            ("role".to_owned(), "path-role".to_owned()),
            ("role_grant".to_owned(), "grant".to_owned()),
        ];
        for posted in [
            json!({"permission": "a"}),
            json!({"role": "other", "permission": "a"}),
        ] {
            let mut posted = posted;

            bind_parents(&mut posted, &parameters, &[ROLE]);

            assert_eq!(posted, json!({"role": "path-role", "permission": "a"}));
        }
    }
}
