use sofie_core::{Error, Result, SqlValue};

use crate::Aggregate;
use crate::aggregate::table;
use crate::requested::{STAMP_REQUESTED, stored_mut};

pub fn delete_softly<A: Aggregate>(aggregate: &mut A) -> Result<()> {
    if !table::<A>().hiding().any(|column| column == "deleted_at") {
        return Err(Error::Rule(format!(
            "a {} cannot be deleted: it keeps no deleted records",
            A::NAME
        )));
    }
    let deleted_at = stored_mut(aggregate, "deleted_at");
    if *deleted_at == SqlValue::Null {
        *deleted_at = STAMP_REQUESTED;
    }
    Ok(())
}
