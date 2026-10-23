use sofie_core::{Cast, Error, Instant, Result, SqlValue};

use crate::behavior::{stored, stored_time};
use crate::requested::{STAMP_REQUESTED, stamp_requested, stored_mut};
use crate::{Aggregate, Behavior, Column, Field, Plan, Uses, Write};

pub struct PublishLock;

impl Behavior for PublishLock {
    fn columns(&self) -> &'static [Field] {
        const { &[Field::new("published_at", Instant::COLUMNS, true)] }
    }

    fn written(&self, write: &Write<'_>, values: &mut [SqlValue]) {
        stamp_requested(write, &mut values[0]);
    }

    fn planned(&self, write: &Write<'_>, _plan: &mut Plan<'_>) -> Result<()> {
        let published = write
            .loaded("published_at")
            .is_some_and(|value| *value != SqlValue::Null);
        if published && !write.changed_fields().is_empty() {
            return Err(Error::Rule(format!(
                "a published {} cannot be changed; supersede it with a new one",
                write.table.name
            )));
        }
        Ok(())
    }
}

pub trait Publishes: Aggregate {
    const PUBLISHED_AT: Column<Self, Option<Instant>> = Column::new("published_at");

    fn publish(&mut self) {
        if !self.is_published() {
            *stored_mut::<Self>(self, "published_at") = STAMP_REQUESTED;
        }
    }

    fn is_published(&self) -> bool {
        stored::<Self>(self, "published_at").is_some_and(|value| *value != SqlValue::Null)
    }

    fn published_at(&self) -> Option<Instant> {
        stored_time::<Self>(self, "published_at")
    }
}

impl<A: Uses<PublishLock>> Publishes for A {}

/// Sent through the message table, so it is delivered after the commit.
pub struct Broadcasts;

impl Behavior for Broadcasts {
    fn columns(&self) -> &'static [Field] {
        &[]
    }

    fn written(&self, _write: &Write<'_>, _values: &mut [SqlValue]) {}

    fn broadcasts(&self) -> bool {
        true
    }
}
