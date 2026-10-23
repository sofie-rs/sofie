use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sofie_core::{Cast, ColumnReader, ColumnSpec, Result, Show, Shown, SqlValue, Uuid};
use sofie_values::{Input, Present};

pub struct Id<A> {
    uuid: Uuid,
    aggregate: PhantomData<fn() -> A>,
}

impl<A> Id<A> {
    #[must_use]
    pub const fn from_uuid(uuid: Uuid) -> Self {
        Self {
            uuid,
            aggregate: PhantomData,
        }
    }

    #[must_use]
    pub const fn uuid(self) -> Uuid {
        self.uuid
    }
}

impl<A> Clone for Id<A> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<A> Copy for Id<A> {}

impl<A> sofie_core::Ordered for Id<A> {}

impl<A> PartialEq for Id<A> {
    fn eq(&self, other: &Self) -> bool {
        self.uuid == other.uuid
    }
}

impl<A> Eq for Id<A> {}

impl<A> Hash for Id<A> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.uuid.hash(state);
    }
}

impl<A> fmt::Debug for Id<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.uuid, f)
    }
}

impl<A> fmt::Display for Id<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.uuid, f)
    }
}

impl<A> Cast for Id<A> {
    const COLUMNS: &'static [ColumnSpec] = Uuid::COLUMNS;

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.uuid.encode(out);
    }

    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        Uuid::decode(row).map(Self::from_uuid)
    }
}

impl<A> Show for Id<A> {
    fn show(&self) -> Shown<'_> {
        Shown::Uuid(self.uuid)
    }
}

impl<A> Serialize for Id<A> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.uuid)
    }
}

impl<'de, A> Deserialize<'de> for Id<A> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Uuid::deserialize(deserializer).map(Self::from_uuid)
    }
}

impl<A> Present for Id<A> {
    const INPUT: Input = Uuid::INPUT;
    const SEARCH: sofie_values::Search = Uuid::SEARCH;

    fn csv(&self) -> String {
        self.uuid.csv()
    }
}
