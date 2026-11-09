use std::sync::Arc;

use sofie_core::{Cast, ColumnReader, Result, SqlType, SqlValue, Uuid};

use super::SEARCH_COLUMN;
use crate::{Db, Migrations, Read, Step, query_decoded};

pub const REINDEX_TASK: &str = "sofie.search.reindex";
pub(super) const PAGE: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchIndex {
    pub(crate) module: &'static str,
    pub(crate) name: &'static str,
    pub(crate) owner: Option<&'static str>,
    pub(crate) hidden: Option<&'static str>,
    pub(crate) fields: &'static [crate::SearchedField],
}

/// `text` is the record's search text, recomputed from the fields `Searched` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    pub id: Uuid,
    pub text: String,
    pub owner: Option<Uuid>,
}

#[derive(Clone)]
pub struct SearchTexts(Arc<[SearchIndex]>);

impl SearchTexts {
    #[must_use]
    pub fn of(migrations: &[Migrations]) -> Self {
        Self(
            migrations
                .iter()
                .flat_map(|migrations| migrations.aggregates)
                .filter_map(crate::AggregateTable::search_index)
                .collect(),
        )
    }

    /// Without a search engine the database matches `search_text` itself, so a record written
    /// before a field became searched is found by it only once this rewrites its text.
    #[cfg(not(feature = "search-engine"))]
    pub async fn reindex(&self, read: Read) -> Result<()> {
        self.rewrite(&read.db).await
    }

    #[cfg(feature = "search-engine")]
    pub async fn reindex(&self, read: Read) -> Result<super::Reindexing> {
        if let Some(installed) = read.db.search.get() {
            return Ok(super::Reindexing::every_page(&installed.indexes));
        }
        self.rewrite(&read.db).await?;
        Ok(super::Reindexing::every_page(&[]))
    }

    async fn rewrite(&self, db: &Db) -> Result<()> {
        for index in self.0.iter() {
            let mut after = None;
            loop {
                let records = read_records(db, index, Records::After(after)).await?;
                rewrite_stale(db, index, &records).await?;
                if records.len() < PAGE {
                    break;
                }
                after = records.last().map(|record| record.document.id);
            }
        }
        Ok(())
    }
}

/// Each stored `search_text` that differs from the recomputed text, unless the record changed
/// since it was read: that commit wrote its own.
pub(super) async fn rewrite_stale(db: &Db, index: &SearchIndex, records: &[Record]) -> Result<()> {
    let dialect = db.driver().dialect();
    let sql = format!(
        "UPDATE {} SET {} = {} WHERE {} = {} AND {} = {}",
        dialect.qualified(index.module, index.name),
        dialect.quote(SEARCH_COLUMN),
        dialect.placeholder(1),
        dialect.quote("id"),
        dialect.placeholder(2),
        dialect.quote("version"),
        dialect.placeholder(3),
    );
    let steps: Vec<Step> = records
        .iter()
        .filter(|record| record.stale)
        .map(|record| {
            let params = vec![
                SqlValue::Text(record.document.text.clone()),
                SqlValue::Uuid(record.document.id),
                record.version.clone(),
            ];
            Step::statement(sql.clone(), params, None)
        })
        .collect();
    if steps.is_empty() {
        return Ok(());
    }
    db.driver().transaction(steps).await
}

pub(super) enum Records {
    #[cfg(feature = "search-engine")]
    One(Uuid),
    After(Option<Uuid>),
}

pub(super) struct Record {
    pub(super) document: Document,
    #[cfg_attr(
        not(feature = "search-engine"),
        expect(dead_code, reason = "only an engine leaves hidden records out")
    )]
    pub(super) hidden: bool,
    pub(super) version: SqlValue,
    stale: bool,
}

/// Deleted records too, so a job can remove them. The text is recomputed from the fields searched
/// now, so one a field no longer searched (made `#[sensitive]`) still held leaves the engine.
pub(super) async fn read_records(
    db: &Db,
    index: &SearchIndex,
    records: Records,
) -> Result<Vec<Record>> {
    let dialect = db.driver().dialect();
    let text_columns = super::text_columns(index.fields);
    let mut columns = vec!["id", "version", SEARCH_COLUMN];
    let mut types = vec![Some(SqlType::Uuid), Some(SqlType::Int), Some(SqlType::Text)];
    if let Some(owner) = index.owner {
        columns.push(owner);
        types.push(Some(SqlType::Uuid));
    }
    if let Some(hidden) = index.hidden {
        columns.push(hidden);
        types.push(None);
    }
    columns.extend(text_columns.iter().map(|(column, _)| column.as_str()));
    types.extend(text_columns.iter().map(|_| None));
    let selected: Vec<String> = columns.iter().map(|column| dialect.quote(column)).collect();
    let (id, first) = (dialect.quote("id"), dialect.placeholder(1));
    let (rows, params) = match records {
        #[cfg(feature = "search-engine")]
        Records::One(record) => (
            format!("WHERE {id} = {first}"),
            vec![SqlValue::Uuid(record)],
        ),
        Records::After(Some(after)) => (
            format!("WHERE {id} > {first} ORDER BY {id} LIMIT {PAGE}"),
            vec![SqlValue::Uuid(after)],
        ),
        Records::After(None) => (format!("ORDER BY {id} LIMIT {PAGE}"), Vec::new()),
    };
    let sql = format!(
        "SELECT {} FROM {} {rows}",
        selected.join(", "),
        dialect.qualified(index.module, index.name)
    );
    query_decoded(db.driver(), &sql, &params, types, |row| {
        decode_record(index, &text_columns, row)
    })
    .await
}

fn decode_record(
    index: &SearchIndex,
    text_columns: &[(String, SqlType)],
    row: &mut dyn ColumnReader,
) -> Result<Record> {
    let id = Uuid::decode(row)?;
    let version = SqlValue::Int(i64::decode(row)?);
    let stored = String::decode(row)?;
    let owner = match index.owner {
        Some(_) => Option::<Uuid>::decode(row)?,
        None => None,
    };
    let hidden = match index.hidden {
        Some(_) => {
            let hidden = !row.is_null()?;
            row.skip()?;
            hidden
        }
        None => false,
    };
    let values = text_columns
        .iter()
        .map(|(_, ty)| searched_value(*ty, row))
        .collect::<Result<Vec<_>>>()?;
    let text = if index.fields.is_empty() {
        stored.clone()
    } else {
        super::search_text(index.fields, |column| {
            let position = text_columns.iter().position(|(name, _)| name == column)?;
            values.get(position)
        })
    };
    Ok(Record {
        stale: text != stored,
        document: Document { id, text, owner },
        hidden,
        version,
    })
}

/// Only text and ids make words; any other value reads as nothing.
fn searched_value(ty: SqlType, row: &mut dyn ColumnReader) -> Result<SqlValue> {
    if row.is_null()? {
        row.skip()?;
        return Ok(SqlValue::Null);
    }
    match ty {
        SqlType::Text => Ok(SqlValue::Text(row.text()?.to_owned())),
        SqlType::Uuid => Ok(SqlValue::Uuid(row.uuid()?)),
        _ => {
            row.skip()?;
            Ok(SqlValue::Null)
        }
    }
}
