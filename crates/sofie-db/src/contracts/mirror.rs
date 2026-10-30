use std::marker::PhantomData;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use sofie_core::{Error, Instant, Result, SqlValue, Uuid};

use super::{Dep, Lookup};
use crate::aggregate::table;
use crate::change_event::DELETED_AT;
use crate::event::PRUNED;
use crate::worker::JobContext;
use crate::{Aggregate, Context, Event, MessageKind, Outcome, Plan, Resolve};

const PAGE: u64 = 500;

pub trait Mirror: Aggregate {
    /// The source's contract type, whose `Lookup` lists the records a new mirror starts from.
    type Source: Send + Sync + 'static;
    type Changed: Event;

    fn columns(changed: &Map<String, Value>) -> Result<Vec<SqlValue>>;
}

pub fn changed_field<T: DeserializeOwned>(changed: &Map<String, Value>, name: &str) -> Result<T> {
    let value = changed.get(name).cloned().unwrap_or(Value::Null);
    serde_json::from_value(value).map_err(|error| {
        Error::internal(format!("reading field `{name}` of a change event"), error)
    })
}

pub struct MirrorRow<M> {
    payload: Map<String, Value>,
    mirror: PhantomData<fn() -> M>,
}

impl<M> MirrorRow<M> {
    fn of(payload: Map<String, Value>) -> Self {
        Self {
            payload,
            mirror: PhantomData,
        }
    }
}

/// Read untyped: a pruned record's event carries none of the fields `M::Changed` needs.
impl<M: Mirror> Resolve<JobContext> for MirrorRow<M> {
    fn resolve(context: &mut JobContext) -> impl Future<Output = Result<Self>> + Send {
        let row = match context.event(<M::Changed as Event>::NAME) {
            Ok(Value::Object(payload)) => Ok(Self::of(payload)),
            Ok(_) => Err(Error::internal_message(
                "a change event's payload is no object",
            )),
            Err(error) => Err(error),
        };
        std::future::ready(row)
    }
}

/// A backfill locks the whole mirror instead, so the row lock is shared on the mirror.
impl<M: Mirror> Outcome for MirrorRow<M> {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        let id: Uuid = changed_field(&self.payload, "id")?;
        let dialect = plan.dialect();
        if let (Some(shared), Some(exclusive)) = (dialect.shared_key_lock(), dialect.key_lock()) {
            let key = format!("{}.{}:{id}", M::MODULE, M::NAME);
            plan.push(crate::Step::statement(
                shared,
                vec![mirror_name::<M>()],
                None,
            ));
            plan.push(crate::Step::statement(
                exclusive,
                vec![SqlValue::Text(key)],
                None,
            ));
        }
        self.write(id, plan)
    }
}

/// Never overwrites a later version: change events may arrive out of order.
impl<M: Mirror> MirrorRow<M> {
    fn write(&self, id: Uuid, plan: &mut Plan<'_>) -> Result<()> {
        let version: i64 = changed_field(&self.payload, "version")?;
        let deleted_at: Option<Instant> = changed_field(&self.payload, DELETED_AT)?;
        let table = table::<M>();
        let hides = table.stored.iter().any(|column| column == DELETED_AT);
        let pruned = changed_field::<Option<bool>>(&self.payload, PRUNED)? == Some(true);
        if pruned || (deleted_at.is_some() && !hides) {
            plan_forgetting::<M>(id, version, plan);
            return Ok(());
        }
        let now = plan.stamp().now;
        let mut params = vec![SqlValue::Uuid(id)];
        params.extend(table.stored.iter().map(|column| match column.as_str() {
            "version" => SqlValue::Int(version),
            "created_at" | "updated_at" => SqlValue::Timestamp(now.timestamp()),
            DELETED_AT => {
                deleted_at.map_or(SqlValue::Null, |at| SqlValue::Timestamp(at.timestamp()))
            }
            _ => SqlValue::Null,
        }));
        params.extend(M::columns(&self.payload)?);
        let dialect = plan.dialect();
        let columns: Vec<&str> = table.row_columns().collect();
        let updates: Vec<String> = columns[1..]
            .iter()
            .filter(|column| **column != "created_at")
            .map(|column| format!("{0} = excluded.{0}", dialect.quote(column)))
            .collect();
        let qualified = table.qualified(dialect);
        let sql = format!(
            "{} ON CONFLICT (id) DO UPDATE SET {} WHERE {qualified}.{version} < excluded.{version}",
            crate::sql::insert(dialect, &qualified, &columns),
            updates.join(", "),
            version = dialect.quote("version"),
        );
        plan.push_as::<M>(crate::Step::statement(sql, params, None));
        plan.push(unforgotten_dropped::<M>(id, plan.dialect()));
        announce::<M>(id, plan);
        Ok(())
    }
}

fn mirror_name<M: Mirror>() -> SqlValue {
    SqlValue::Text(format!("{}.{}", M::MODULE, M::NAME))
}

/// An older change arriving after its record was dropped is taken back out at once.
fn unforgotten_dropped<M: Mirror>(id: Uuid, dialect: &dyn crate::Dialect) -> crate::Step {
    let sql = format!(
        "DELETE FROM {table} WHERE {id} = {first} AND {version} <= \
         (SELECT {version} FROM {forgotten} WHERE mirror = {second} AND {id} = {first})",
        table = table::<M>().qualified(dialect),
        forgotten = dialect.qualified("sofie", "mirror_forgotten"),
        id = dialect.quote("id"),
        version = dialect.quote("version"),
        first = dialect.placeholder(1),
        second = dialect.placeholder(2),
    );
    crate::Step::statement(sql, vec![SqlValue::Uuid(id), mirror_name::<M>()], None)
}

/// A pruned record, or a deleted one a mirror cannot hide, is dropped: no copy outlives its source.
fn plan_forgetting<M: Mirror>(id: Uuid, version: i64, plan: &mut Plan<'_>) {
    let dialect = plan.dialect();
    let sql = format!(
        "DELETE FROM {} WHERE {} = {} AND {} < {}",
        table::<M>().qualified(dialect),
        dialect.quote("id"),
        dialect.placeholder(1),
        dialect.quote("version"),
        dialect.placeholder(2)
    );
    plan.push_as::<M>(crate::Step::statement(
        sql,
        vec![SqlValue::Uuid(id), SqlValue::Int(version)],
        None,
    ));
    let forgotten = dialect.qualified("sofie", "mirror_forgotten");
    let remembered = format!(
        "INSERT INTO {forgotten} (mirror, {id}, {version}, forgotten_at) VALUES ({}, {}, {}, {}) \
         ON CONFLICT (mirror, {id}) DO UPDATE SET {version} = excluded.{version}, \
         forgotten_at = excluded.forgotten_at WHERE {forgotten}.{version} < excluded.{version}",
        dialect.placeholder(1),
        dialect.placeholder(2),
        dialect.placeholder(3),
        dialect.placeholder(4),
        id = dialect.quote("id"),
        version = dialect.quote("version"),
    );
    let params = vec![
        mirror_name::<M>(),
        SqlValue::Uuid(id),
        SqlValue::Int(version),
        SqlValue::Timestamp(plan.stamp().now.timestamp()),
    ];
    plan.push(crate::Step::statement(remembered, params, None));
    announce::<M>(id, plan);
}

#[cfg_attr(not(feature = "cache"), expect(unused_variables))]
fn announce<M: Mirror>(id: Uuid, plan: &mut Plan<'_>) {
    #[cfg(feature = "cache")]
    plan.announcements.changed(crate::ChangedRecord {
        module: M::MODULE,
        aggregate: M::NAME,
        id: Some(id),
    });
    #[cfg(feature = "live")]
    plan.announcements
        .live_change(crate::live::TableChange::of::<M>(None, plan.reach()));
}

/// Past the longest a late change event is retried for, a forgotten record cannot come back.
const FORGOTTEN_FOR: Duration = Duration::from_hours(30 * 24);
pub(crate) const SWEEP_FORGOTTEN: &str = "sofie.mirror.sweep_forgotten";

pub(crate) async fn sweep_forgotten(swept: SweptForgotten) -> Result<SweptForgotten> {
    Ok(swept)
}

pub(crate) struct SweptForgotten {
    forgotten_for: Duration,
}

impl Resolve<JobContext> for SweptForgotten {
    fn resolve(context: &mut JobContext) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Ok(Self {
            forgotten_for: FORGOTTEN_FOR.max(context.retried_for),
        }))
    }
}

impl Outcome for SweptForgotten {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        let before = plan.stamp().now.checked_sub(self.forgotten_for)?;
        let dialect = plan.dialect();
        let sql = format!(
            "DELETE FROM {} WHERE forgotten_at < {}",
            dialect.qualified("sofie", "mirror_forgotten"),
            dialect.placeholder(1)
        );
        plan.push(crate::Step::statement(
            sql,
            vec![SqlValue::Timestamp(before.timestamp())],
            None,
        ));
        Ok(())
    }
}

pub struct MirrorPage {
    after: Option<Uuid>,
}

impl Resolve<JobContext> for MirrorPage {
    fn resolve(context: &mut JobContext) -> impl Future<Output = Result<Self>> + Send {
        let after = changed_field::<Option<Uuid>>(
            context.payload.as_object().unwrap_or(&Map::new()),
            "after",
        );
        std::future::ready(after.map(|after| Self { after }))
    }
}

pub async fn backfill_mirror<M: Mirror>(
    source: Dep<dyn Lookup<M::Source>>,
    page: MirrorPage,
    job: &'static str,
) -> Result<Backfilled<M>> {
    let changes = source.changes(page.after, PAGE).await?;
    let full = u64::try_from(changes.len()).is_ok_and(|copied| copied == PAGE);
    let last = changes
        .last()
        .map(|change| changed_field::<Uuid>(change, "id"));
    let next = match last {
        Some(last) if full => Some(last?),
        _ => None,
    };
    Ok(Backfilled {
        rows: changes.into_iter().map(MirrorRow::of).collect(),
        next: next.map(|after| (job, after)),
    })
}

pub struct Backfilled<M> {
    rows: Vec<MirrorRow<M>>,
    next: Option<(&'static str, Uuid)>,
}

impl<M: Mirror> Outcome for Backfilled<M> {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        if let Some(lock) = plan.dialect().key_lock() {
            plan.push(crate::Step::statement(lock, vec![mirror_name::<M>()], None));
        }
        for row in &self.rows {
            row.write(changed_field(&row.payload, "id")?, plan)?;
        }
        if let Some((job, after)) = self.next {
            plan.queue(MessageKind::Job, [(job, json!({ "after": after }))])?;
        }
        Ok(())
    }
}
