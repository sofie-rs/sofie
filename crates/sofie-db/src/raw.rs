use std::marker::PhantomData;

use sofie_core::{Cast, Error, Result, SqlValue};

use crate::{BoxFuture, Decoding, Read, RowSink, Source};

/// `'static` text takes input only as parameters.
pub struct Sql<'r, T> {
    read: &'r Read,
    statement: &'static str,
    params: Vec<SqlValue>,
    marker: PhantomData<fn() -> T>,
}

impl Read {
    /// Read-only; an owned area cannot use it, as nothing would keep it to its
    /// principal's records.
    #[must_use]
    pub fn sql<T: Source>(&self, sql: &'static str) -> Sql<'_, T> {
        Sql {
            read: self,
            statement: sql,
            params: Vec::new(),
            marker: PhantomData,
        }
    }
}

impl<T: Source> Sql<'_, T> {
    #[must_use]
    pub fn bind<V: Cast>(mut self, value: &V) -> Self {
        value.encode(&mut self.params);
        self
    }

    pub async fn all(self) -> Result<Vec<T>> {
        if self.read.owner().is_some() {
            return Err(Error::internal_message(format!(
                "raw SQL cannot be kept to one owner's records; read `{}` with the query \
                 builder in an owned area",
                std::any::type_name::<T>()
            )));
        }
        let role = cfg!(feature = "module-roles").then_some(<T::Base as crate::Aggregate>::MODULE);
        let mut rows = Decoding::new(T::column_types(), T::decode);
        self.read
            .query_read_only(role, self.statement, &self.params, &mut rows)
            .await?;
        Ok(rows.into_decoded())
    }
}

pub(crate) fn refuse_read_only<'a>(sql: &str) -> BoxFuture<'a, Result<()>> {
    Box::pin(std::future::ready(Err(Error::internal_message(format!(
        "this database cannot run raw SQL read-only, so `{sql}` was not run"
    )))))
}

impl Read {
    async fn query_read_only(
        &self,
        role: Option<&str>,
        sql: &str,
        params: &[SqlValue],
        rows: &mut dyn RowSink,
    ) -> Result<()> {
        #[cfg(feature = "live")]
        crate::live::read_anything();
        let driver = self.db.driver();
        #[cfg(feature = "etag")]
        if crate::read_versions::tracking() {
            let mut fingerprinted = crate::read_versions::Fingerprinted::new(rows);
            driver
                .query_read_only(role, sql, params, &mut fingerprinted)
                .await?;
            fingerprinted.record();
            return Ok(());
        }
        driver.query_read_only(role, sql, params, rows).await
    }
}
