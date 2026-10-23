use sofie_core::SqlValue;

use crate::Aggregate;
use crate::aggregate::table;

/// The method has no clock, so the commit stamps the time when it writes.
pub(crate) const STAMP_REQUESTED: SqlValue = SqlValue::Bool(true);

#[cfg(any(feature = "publishing", feature = "soft-delete"))]
pub(crate) fn stamp_requested(write: &crate::Write<'_>, value: &mut SqlValue) {
    if *value == STAMP_REQUESTED {
        *value = SqlValue::Timestamp(write.stamp.now.timestamp());
    }
}

pub(crate) fn stored_mut<'a, A: Aggregate>(aggregate: &'a mut A, column: &str) -> &'a mut SqlValue {
    let table = table::<A>();
    let index = table.stored_index(column);
    let stored = &mut aggregate.meta_mut().stored;
    if stored.is_empty() {
        stored.resize(table.stored.len(), SqlValue::Null);
    }
    &mut stored[index]
}
