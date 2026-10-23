use sofie_core::{Error, Result, SqlValue, Timestamp};

use crate::{BoxFuture, Db, Step, Wakeups, query_values};

/// `notify` is sent on every change.
pub struct ShieldSql {
    pub upsert: &'static str,
    pub delete: &'static str,
    pub active: &'static str,
    pub notify: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedBlock {
    pub key: String,
    pub rule: String,
    pub grade: String,
    pub until: Timestamp,
}

pub struct SharedBlocks<'d> {
    db: &'d Db,
}

impl<'d> SharedBlocks<'d> {
    #[must_use]
    pub fn of(db: &'d Db) -> Self {
        Self { db }
    }

    fn sql(&self) -> Result<&'static ShieldSql> {
        self.db
            .driver()
            .dialect()
            .shield()
            .ok_or_else(|| Error::internal_message("this database keeps no shared shield blocks"))
    }

    pub async fn share(&self, block: &SharedBlock) -> Result<()> {
        let sql = self.sql()?;
        let params = vec![
            SqlValue::Text(block.key.clone()),
            SqlValue::Text(block.rule.clone()),
            SqlValue::Text(block.grade.clone()),
            SqlValue::Timestamp(block.until),
        ];
        self.db
            .transaction(vec![Step::statement(sql.upsert, params, None), notify(sql)])
            .await
    }

    pub async fn lift(&self, key: &str) -> Result<()> {
        let sql = self.sql()?;
        let params = vec![SqlValue::Text(key.to_owned())];
        self.db
            .transaction(vec![Step::statement(sql.delete, params, None), notify(sql)])
            .await
    }

    pub async fn active(&self, now: Timestamp) -> Result<Vec<SharedBlock>> {
        let params = [SqlValue::Timestamp(now)];
        let rows = query_values(self.db.driver(), self.sql()?.active, &params).await?;
        rows.into_iter().map(shared_block).collect()
    }

    #[must_use]
    pub fn wakeups(&self) -> Option<BoxFuture<'d, Result<Box<dyn Wakeups>>>> {
        self.db.driver().shield_wakeups()
    }
}

fn notify(sql: &ShieldSql) -> Step {
    Step::statement(sql.notify, Vec::new(), None)
}

fn shared_block(row: Vec<SqlValue>) -> Result<SharedBlock> {
    match <[SqlValue; 4]>::try_from(row) {
        Ok(
            [
                SqlValue::Text(key),
                SqlValue::Text(rule),
                SqlValue::Text(grade),
                SqlValue::Timestamp(until),
            ],
        ) => Ok(SharedBlock {
            key,
            rule,
            grade,
            until,
        }),
        _ => Err(Error::internal_message(
            "a shared shield block is not `key, rule, grade, until`",
        )),
    }
}
