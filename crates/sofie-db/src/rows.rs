use sofie_core::{ColumnReader, ColumnSpec, Result, SqlType, SqlValue};

use crate::{Driver, RowSink};

pub struct Decoding<T, F> {
    expects: Vec<Option<SqlType>>,
    decode: F,
    decoded: Vec<T>,
}

impl<T, F> Decoding<T, F>
where
    T: Send,
    F: FnMut(&mut dyn ColumnReader) -> Result<T> + Send,
{
    pub fn new(expects: Vec<Option<SqlType>>, decode: F) -> Self {
        Self {
            expects,
            decode,
            decoded: Vec::new(),
        }
    }

    pub(crate) fn reserve(&mut self, rows: usize) {
        self.decoded.reserve(rows);
    }

    pub fn into_decoded(self) -> Vec<T> {
        self.decoded
    }
}

impl<T, F> RowSink for Decoding<T, F>
where
    T: Send,
    F: FnMut(&mut dyn ColumnReader) -> Result<T> + Send,
{
    fn expects(&self) -> &[Option<SqlType>] {
        &self.expects
    }

    fn row(&mut self, row: &mut dyn ColumnReader) -> Result<()> {
        self.decoded.push((self.decode)(row)?);
        Ok(())
    }
}

pub async fn query_decoded<T: Send>(
    driver: &dyn Driver,
    sql: &str,
    params: &[SqlValue],
    expects: Vec<Option<SqlType>>,
    decode: impl FnMut(&mut dyn ColumnReader) -> Result<T> + Send,
) -> Result<Vec<T>> {
    let mut rows = Decoding::new(expects, decode);
    driver.query(sql, params, &mut rows).await?;
    Ok(rows.into_decoded())
}

/// Each row as owned values, for the few reads that have no type to decode into.
pub async fn query_values(
    driver: &dyn Driver,
    sql: &str,
    params: &[SqlValue],
) -> Result<Vec<Vec<SqlValue>>> {
    query_decoded(driver, sql, params, Vec::new(), row_values).await
}

pub fn row_values(row: &mut dyn ColumnReader) -> Result<Vec<SqlValue>> {
    (0..row.remaining()).map(|_| row.value()).collect()
}

#[must_use]
pub fn column_types(fields: &[&[ColumnSpec]]) -> Vec<Option<SqlType>> {
    fields
        .iter()
        .flat_map(|columns| columns.iter().map(|column| Some(column.ty)))
        .collect()
}
