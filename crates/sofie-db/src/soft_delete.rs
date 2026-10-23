use sofie_core::{Cast, Instant, SqlValue};

use crate::behavior::{stored, stored_time};
use crate::requested::{STAMP_REQUESTED, stamp_requested, stored_mut};
use crate::{Aggregate, Behavior, Column, Field, Uses, Write};

pub struct SoftDelete;

impl Behavior for SoftDelete {
    fn columns(&self) -> &'static [Field] {
        const { &[Field::new("deleted_at", Instant::COLUMNS, true)] }
    }

    fn written(&self, write: &Write<'_>, values: &mut [SqlValue]) {
        stamp_requested(write, &mut values[0]);
    }

    fn hides(&self) -> Option<&'static str> {
        Some("deleted_at")
    }
}

pub trait SoftDeletes: Aggregate {
    const DELETED_AT: Column<Self, Option<Instant>> = Column::new("deleted_at");

    fn delete(&mut self) {
        if !self.is_deleted() {
            *stored_mut::<Self>(self, "deleted_at") = STAMP_REQUESTED;
        }
    }

    fn restore(&mut self) {
        *stored_mut::<Self>(self, "deleted_at") = SqlValue::Null;
    }

    fn is_deleted(&self) -> bool {
        stored::<Self>(self, "deleted_at").is_some_and(|value| *value != SqlValue::Null)
    }

    fn deleted_at(&self) -> Option<Instant> {
        stored_time::<Self>(self, "deleted_at")
    }
}

impl<A: Uses<SoftDelete>> SoftDeletes for A {}
