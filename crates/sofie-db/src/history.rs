use sofie_core::variants::{self, Variants};
use sofie_core::{Cast, ColumnReader, Error, Instant, Result, SqlType, SqlValue, Uuid};

use crate::behavior::stored;
use crate::{
    Aggregate, Behavior, Column, Dialect, Field, Id, Plan, Read, Select, Step, Uses, Write,
};

/// Null for a record written for no principal, such as by a job.
pub struct Audited;

impl Behavior for Audited {
    fn columns(&self) -> &'static [Field] {
        const {
            &[
                Field::new("created_by", Uuid::COLUMNS, true),
                Field::new("updated_by", Uuid::COLUMNS, true),
            ]
        }
    }

    fn written(&self, write: &Write<'_>, values: &mut [SqlValue]) {
        let actor = write.stamp.actor.map_or(SqlValue::Null, SqlValue::Uuid);
        values[0] = write.loaded("created_by").unwrap_or(&actor).clone();
        values[1] = actor;
    }
}

pub trait Authored: Aggregate {
    const CREATED_BY: Column<Self, Option<Uuid>> = Column::new("created_by");
    const UPDATED_BY: Column<Self, Option<Uuid>> = Column::new("updated_by");

    fn created_by(&self) -> Option<Uuid> {
        principal::<Self>(self, "created_by")
    }

    fn updated_by(&self) -> Option<Uuid> {
        principal::<Self>(self, "updated_by")
    }
}

impl<A: Uses<Audited>> Authored for A {}

fn principal<A: Aggregate>(aggregate: &A, column: &str) -> Option<Uuid> {
    match stored(aggregate, column) {
        Some(SqlValue::Uuid(principal)) => Some(*principal),
        _ => None,
    }
}

pub struct RecordsChanges;

impl Behavior for RecordsChanges {
    fn columns(&self) -> &'static [Field] {
        &[]
    }

    fn written(&self, _write: &Write<'_>, _values: &mut [SqlValue]) {}

    fn planned(&self, write: &Write<'_>, plan: &mut Plan<'_>) -> Result<()> {
        let dialect = plan.dialect();
        let fields: Vec<serde_json::Value> = write
            .changed_fields()
            .into_iter()
            .map(serde_json::Value::from)
            .collect();
        let columns = [
            "id",
            "record_table",
            "record",
            "owner",
            "change",
            "fields",
            "actor",
            "action",
            "changed_at",
        ];
        let params = vec![
            SqlValue::Uuid(Uuid::now_v7()?),
            SqlValue::Text(record_table(write.table.schema, write.table.name)),
            SqlValue::Uuid(write.id),
            write.owner.map_or(SqlValue::Null, SqlValue::Uuid),
            SqlValue::Text(ChangeKind::of(write).name().to_owned()),
            SqlValue::Json(fields.into()),
            write.stamp.actor.map_or(SqlValue::Null, SqlValue::Uuid),
            write
                .stamp
                .action
                .map_or(SqlValue::Null, |action| SqlValue::Text(action.to_owned())),
            SqlValue::Timestamp(write.stamp.now.timestamp()),
        ];
        let table = history_table(dialect);
        let sql = crate::sql::insert(dialect, &table, &columns);
        plan.push(Step::statement(sql, params, None));
        Ok(())
    }
}

fn history_table(dialect: &dyn Dialect) -> String {
    dialect.qualified("sofie", "history")
}

fn record_table(schema: &str, name: &str) -> String {
    format!("{schema}.{name}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Created,
    Updated,
    Deleted,
    Restored,
}

impl ChangeKind {
    fn of(write: &Write<'_>) -> Self {
        if write.loaded.is_none() {
            return Self::Created;
        }
        match write.deleted() {
            (false, true) => Self::Deleted,
            (true, false) => Self::Restored,
            _ => Self::Updated,
        }
    }
}

impl Variants for ChangeKind {
    const TYPE: &'static str = "history change";
    const VARIANTS: &'static [(&'static str, Self)] = &[
        ("CREATED", Self::Created),
        ("UPDATED", Self::Updated),
        ("DELETED", Self::Deleted),
        ("RESTORED", Self::Restored),
    ];
}

#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub kind: ChangeKind,
    pub fields: Vec<String>,
    pub actor: Option<Uuid>,
    pub action: Option<String>,
    pub at: Instant,
}

/// An owned area reads the history of a record its principal owns now, through a parent too.
pub trait History: Uses<RecordsChanges> {
    fn history(read: &Read, id: Id<Self>) -> impl Future<Output = Result<Vec<Change>>> + Send {
        history_of::<Self>(read, id)
    }
}

pub async fn history_of<A: Aggregate>(read: &Read, id: Id<A>) -> Result<Vec<Change>> {
    if read.owner().is_some() && !reachable(read, id).await? {
        return Ok(Vec::new());
    }
    let dialect = read.dialect();
    let params = vec![
        SqlValue::Text(record_table(A::MODULE, A::NAME)),
        SqlValue::Uuid(id.uuid()),
    ];
    let sql = format!(
        "SELECT change, fields, actor, action, changed_at FROM {} \
         WHERE record_table = {} AND record = {} ORDER BY changed_at, id",
        history_table(dialect),
        dialect.placeholder(1),
        dialect.placeholder(2)
    );
    #[cfg(feature = "live")]
    crate::live::read_tables([(A::MODULE, A::NAME)]);
    let types = vec![
        Some(SqlType::Text),
        None,
        Some(SqlType::Uuid),
        Some(SqlType::Text),
        Some(SqlType::Timestamp),
    ];
    read.decoded(&sql, &params, 0, types, change).await
}

async fn reachable<A: Aggregate>(read: &Read, id: Id<A>) -> Result<bool> {
    let mut query = A::query().where_(A::ID.eq(id));
    query.criteria.with_hidden = true;
    query.exists(read).await
}

impl<A: Uses<RecordsChanges>> History for A {}

fn change(row: &mut dyn ColumnReader) -> Result<Change> {
    let kind = variants::decode(row)?;
    let serde_json::Value::Array(fields) = row.json()? else {
        return Err(Error::internal_message(
            "expected the changed fields as a JSON array",
        ));
    };
    Ok(Change {
        kind,
        fields: fields
            .into_iter()
            .filter_map(|field| field.as_str().map(str::to_owned))
            .collect(),
        actor: Option::<Uuid>::decode(row)?,
        action: Option::<String>::decode(row)?,
        at: Instant::decode(row)?,
    })
}
