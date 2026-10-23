use rusqlite::Connection;
use sofie_core::{Result, SqlValue};
use sofie_db::{BoxFuture, Catalog, CatalogColumn};

use crate::Sqlite;
use crate::rows::declared_type;

const COLUMNS: &str = "SELECT name, type, \"notnull\", dflt_value IS NOT NULL OR pk > 0 \
    FROM pragma_table_info(?1)";

impl Catalog for Sqlite {
    fn columns<'a>(
        &'a self,
        schema: &'a str,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Vec<CatalogColumn>>> {
        let table = format!("{schema}_{name}");
        let params = [SqlValue::Text(table)];
        Box::pin(async move {
            sofie_db::query_values(self, COLUMNS, &params)
                .await?
                .into_iter()
                .map(column)
                .collect()
        })
    }
}

fn column(row: Vec<SqlValue>) -> Result<CatalogColumn> {
    match <[SqlValue; 4]>::try_from(row) {
        Ok(
            [
                SqlValue::Text(name),
                SqlValue::Text(type_name),
                SqlValue::Int(not_null),
                SqlValue::Int(filled),
            ],
        ) => Ok(CatalogColumn {
            ty: declared_type(Some(&type_name)),
            name,
            type_name,
            nullable: not_null == 0,
            filled: filled != 0,
        }),
        row => Err(sofie_core::Error::internal_message(format!(
            "unexpected row describing a column: {row:?}"
        ))),
    }
}

/// One a column's `UNIQUE` made is named as Postgres would name it.
pub(crate) fn unique_index(
    connection: &Connection,
    columns: &[(String, String)],
) -> Option<String> {
    let table = &columns.first()?.0;
    let wanted: Vec<&str> = columns.iter().map(|(_, column)| column.as_str()).collect();
    let mut indexes = connection
        .prepare("SELECT name, origin FROM pragma_index_list(?1) WHERE \"unique\"")
        .ok()?;
    let listed: Vec<(String, String)> = indexes
        .query_map([table], |row| Ok((row.get(0)?, row.get(1)?)))
        .ok()?
        .collect::<rusqlite::Result<_>>()
        .ok()?;
    let (name, origin) = listed.into_iter().find(|(index, _)| {
        index_columns(connection, index).is_some_and(|indexed| indexed == wanted)
    })?;
    if origin == "c" {
        return Some(name);
    }
    Some(format!("{table}_{}_key", wanted.join("_")))
}

fn index_columns(connection: &Connection, index: &str) -> Option<Vec<String>> {
    let mut columns = connection
        .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")
        .ok()?;
    columns
        .query_map([index], |row| row.get(0))
        .ok()?
        .collect::<rusqlite::Result<_>>()
        .ok()
}
