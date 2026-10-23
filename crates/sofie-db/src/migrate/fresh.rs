use sofie_core::{Error, Result, SqlValue};

use super::{Migrations, Schema};
use crate::{Step, query_values};

impl Schema<'_> {
    /// Schemas no migration made are left alone; without schemas, every table goes.
    pub async fn fresh(self, modules: &[Migrations]) -> Result<()> {
        let drops = match self.db.driver().dialect().drop_every_table() {
            Some(statements) => self.table_drops(statements).await?,
            None => self.schema_drops(modules),
        };
        self.db.transaction(drops).await?;
        self.migrate(modules).await
    }

    fn schema_drops(self, modules: &[Migrations]) -> Vec<Step> {
        let dialect = self.db.driver().dialect();
        let framework = dialect.framework_migrations();
        std::iter::once(&framework)
            .chain(modules)
            .map(|migrations| {
                let schema = dialect.quote(migrations.module);
                Step::Script(format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
            })
            .collect()
    }

    async fn table_drops(self, statements: &str) -> Result<Vec<Step>> {
        query_values(self.db.driver(), statements, &[])
            .await?
            .into_iter()
            .map(|row| match <[SqlValue; 1]>::try_from(row) {
                Ok([SqlValue::Text(statement)]) => Ok(Step::Script(statement)),
                row => Err(Error::internal_message(format!(
                    "the statements dropping every table came back as {row:?}"
                ))),
            })
            .collect()
    }
}
