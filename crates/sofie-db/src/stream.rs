use std::collections::VecDeque;

use sofie_core::{Cast, Error, Result, SqlType, SqlValue, Uuid};

use crate::condition::Kind;
use crate::statement::{Criteria, SortKey};
use crate::{Read, Selected, Source};

const BATCH: usize = 500;

/// One short query per batch, so no cursor or transaction stays open between them.
pub struct Stream<T: Source> {
    criteria: Criteria,
    read: Read,
    batch: VecDeque<T>,
    after: Option<Uuid>,
    exhausted: bool,
}

impl<T: Source> Stream<T> {
    pub(crate) fn new(criteria: Criteria, read: &Read) -> Result<Self> {
        if !criteria.sorts.is_empty() || criteria.limit.is_some() {
            return Err(Error::internal_message(
                "a stream reads every matching record in id order; remove its `order_by`, \
                 `latest` and `limit`",
            ));
        }
        Ok(Self {
            criteria,
            read: read.clone(),
            batch: VecDeque::new(),
            after: None,
            exhausted: false,
        })
    }

    pub async fn next(&mut self) -> Result<Option<T>> {
        if self.batch.is_empty() && !self.exhausted {
            self.read_batch().await?;
        }
        Ok(self.batch.pop_front())
    }

    async fn read_batch(&mut self) -> Result<()> {
        if self.after.is_none() {
            self.criteria = std::mem::take(&mut self.criteria)
                .matched(&self.read)
                .await?;
        }
        let mut criteria = self.criteria.clone();
        if let Some(after) = self.after {
            criteria.conditions.push(Kind::Compare {
                columns: vec!["id".to_owned()],
                operator: ">",
                values: vec![SqlValue::Uuid(after)],
            });
        }
        criteria.sorts.push(SortKey::id(false));
        criteria.limit = u64::try_from(BATCH).ok();
        let mut columns = vec![Selected::column("id")];
        columns.extend(T::columns());
        let (sql, params) =
            criteria.select_sql::<T::Base>(self.read.dialect(), self.read.owner(), &columns);
        let mut types = vec![Some(SqlType::Uuid)];
        types.extend(T::column_types());
        let rows = self
            .read
            .decoded(&sql, &params, BATCH, types, |row| {
                Ok((Uuid::decode(row)?, T::decode(row)?))
            })
            .await?;
        self.exhausted = rows.len() < BATCH;
        for (id, item) in rows {
            self.after = Some(id);
            self.batch.push_back(item);
        }
        Ok(())
    }
}
