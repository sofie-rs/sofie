use sofie_core::{Error, Result, SqlType, SqlValue};
use sofie_db::{BoxFuture, Catalog, CatalogColumn};

use tokio_postgres::types::Type;

use crate::Postgres;
use crate::rows::Wire;

const COLUMNS: &str = "SELECT a.attname::text, t.typname::text, a.atttypid::int8, NOT a.attnotnull, \
    a.atthasdef OR a.attidentity <> '' OR a.attgenerated <> '' \
    FROM pg_catalog.pg_attribute a \
    JOIN pg_catalog.pg_class c ON c.oid = a.attrelid \
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
    JOIN pg_catalog.pg_type t ON t.oid = a.atttypid \
    WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind IN ('r', 'p') \
    AND a.attnum > 0 AND NOT a.attisdropped \
    ORDER BY a.attnum";

impl Catalog for Postgres {
    fn columns<'a>(
        &'a self,
        schema: &'a str,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Vec<CatalogColumn>>> {
        Box::pin(async move {
            let params = [
                SqlValue::Text(schema.to_owned()),
                SqlValue::Text(name.to_owned()),
            ];
            sofie_db::query_values(self, COLUMNS, &params)
                .await?
                .into_iter()
                .map(column)
                .collect()
        })
    }
}

fn column(row: Vec<SqlValue>) -> Result<CatalogColumn> {
    match <[SqlValue; 5]>::try_from(row) {
        Ok(
            [
                SqlValue::Text(name),
                SqlValue::Text(type_name),
                SqlValue::Int(type_oid),
                SqlValue::Bool(nullable),
                SqlValue::Bool(filled),
            ],
        ) => Ok(CatalogColumn {
            name,
            ty: sql_type(type_oid, &type_name),
            type_name,
            nullable,
            filled,
        }),
        row => Err(Error::internal_message(format!(
            "unexpected row describing a column: {row:?}"
        ))),
    }
}

/// Read as rows read it, so the schema check and a query agree on what a column holds.
fn sql_type(type_oid: i64, type_name: &str) -> Option<SqlType> {
    let known = u32::try_from(type_oid).ok().and_then(Type::from_oid);
    known
        .map_or_else(|| Wire::named(type_name), |ty| Wire::of(&ty))
        .sql_type()
}
