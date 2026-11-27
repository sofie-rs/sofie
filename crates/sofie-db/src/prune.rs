use sofie_core::{Error, Ordered, Result, SqlValue};

use crate::heard::Listeners;
use crate::worker::JobContext;
use crate::{Aggregate, Column, Context, Db, Dialect, Outcome, Plan, Resolve, Select, Step};

const BATCH: u64 = 1000;

/// Taken by a job, as `async fn forget_audits(prune: Prune, clock: Clock)`.
pub struct Prune {
    db: Db,
}

impl Resolve<JobContext> for Prune {
    fn resolve(context: &mut JobContext) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Ok(Self {
            db: context.db().clone(),
        }))
    }
}

impl Prune {
    #[cfg(feature = "product-reports")]
    pub(crate) fn db(&self) -> &Db {
        &self.db
    }

    /// In batches, so no statement holds many rows' locks and a retried job resumes; nulls stay.
    pub async fn before<A: Aggregate, T: Ordered + Clone + Send>(
        &self,
        column: Column<A, T>,
        before: T,
    ) -> Result<u64> {
        let mut deleted = 0;
        loop {
            let mut query = A::query()
                .where_(column.lt(before.clone()))
                .order_by(column.asc())
                .limit(BATCH);
            query.criteria.with_hidden = true;
            let records = query.all(&self.db.read()).await?;
            let batch = u64::try_from(records.len())
                .map_err(|error| Error::internal("counting pruned records", error))?;
            self.db.commit(&mut Pruned { records }).await?;
            deleted += batch;
            if batch < BATCH {
                return Ok(deleted);
            }
        }
    }
}

#[cfg_attr(
    not(feature = "contracts"),
    expect(unused_variables, clippy::unnecessary_wraps)
)]
fn heard_events<A: Aggregate>(first: &A, heard: &Listeners) -> Result<Events> {
    Ok(Events {
        #[cfg(feature = "contracts")]
        change: change_event(first, heard)?,
        pruned: A::PRUNED.filter(|name| heard.hear(name)),
    })
}

#[cfg(feature = "contracts")]
fn change_event<A: Aggregate>(first: &A, heard: &Listeners) -> Result<Option<&'static str>> {
    match A::CHANGED {
        Some(name) if heard.hear_pruned(name) => {
            Ok(crate::change_event::changes(first)?.then_some(name))
        }
        _ => Ok(None),
    }
}

#[derive(Default)]
struct Events {
    #[cfg(feature = "contracts")]
    change: Option<&'static str>,
    pruned: Option<&'static str>,
}

struct Pruned<A> {
    records: Vec<A>,
}

impl<A: Aggregate> Outcome for Pruned<A> {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        plan_deletion(&self.records, plan)
    }
}

/// Each record only at the version it was loaded at: one changed since is a conflict.
pub(crate) fn plan_deletion<A: Aggregate>(records: &[A], plan: &mut Plan<'_>) -> Result<()> {
    let Some(first) = records.first() else {
        return Ok(());
    };
    let events = match plan.heard.as_deref() {
        Some(known) => heard_events(first, &known.listeners)?,
        None => Events::default(),
    };
    let ids: Vec<sofie_core::Uuid> = records.iter().map(|record| record.meta().id()).collect();
    #[cfg(feature = "children")]
    crate::children::delete_children::<A>(&ids, plan);
    plan.push_as::<A>(delete_loaded::<A>(records, plan.dialect())?);
    let table = crate::aggregate::table::<A>();
    for (behavior, _) in &table.behaviors {
        behavior.pruned(table.schema, table.name, &ids, plan)?;
    }
    for record in records {
        announce_deleted(record, &events, plan);
    }
    Ok(())
}

fn announce_deleted<A: Aggregate>(record: &A, events: &Events, plan: &mut Plan<'_>) {
    let meta = record.meta();
    #[cfg(feature = "contracts")]
    if let Some(name) = events.change {
        plan.events.push(crate::RaisedEvent {
            name,
            payload: crate::change_event::pruned_change(meta),
        });
    }
    if let Some(name) = events.pruned {
        let pruned =
            crate::event::change_payload(serde_json::Map::new(), meta.id(), meta.version() + 1);
        plan.events.push(crate::RaisedEvent {
            name,
            payload: serde_json::Value::Object(pruned),
        });
    }
    #[cfg(feature = "cache")]
    plan.announcements.changed(crate::ChangedRecord {
        module: A::MODULE,
        aggregate: A::NAME,
        id: Some(meta.id()),
    });
    #[cfg(feature = "live")]
    plan.announcements
        .live_change(crate::TableChange::of::<A>(record.owner(), plan.reach()));
}

fn delete_loaded<A: Aggregate>(records: &[A], dialect: &dyn Dialect) -> Result<Step> {
    let table = crate::aggregate::table::<A>().qualified(dialect);
    let (id, version) = (dialect.quote("id"), dialect.quote("version"));
    let expected = u64::try_from(records.len())
        .map_err(|error| Error::internal("counting pruned records", error))?;
    if dialect.binds_arrays() {
        // One statement text whatever the batch size, so each connection prepares it once.
        let sql = format!(
            "DELETE FROM {table} WHERE ({id}, {version}) IN \
             (SELECT * FROM unnest({}::uuid[], {}::bigint[]))",
            dialect.placeholder(1),
            dialect.placeholder(2)
        );
        let ids = records
            .iter()
            .map(|record| SqlValue::Uuid(record.meta().id()))
            .collect();
        let versions = records
            .iter()
            .map(|record| SqlValue::Int(record.meta().version()))
            .collect();
        let params = vec![SqlValue::Array(ids), SqlValue::Array(versions)];
        return Ok(Step::statement(sql, params, Some(expected)));
    }
    let mut params = Vec::with_capacity(records.len() * 2);
    let mut matches = Vec::with_capacity(records.len());
    for record in records {
        let meta = record.meta();
        params.extend([SqlValue::Uuid(meta.id()), SqlValue::Int(meta.version())]);
        matches.push(format!(
            "({id} = {} AND {version} = {})",
            dialect.placeholder(params.len() - 1),
            dialect.placeholder(params.len())
        ));
    }
    let sql = format!("DELETE FROM {table} WHERE {}", matches.join(" OR "));
    Ok(Step::statement(sql, params, Some(expected)))
}
