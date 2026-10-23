use sofie_core::{Cast, ColumnReader, Result, SqlValue, Timestamp, Uuid};
use sofie_db::{Aggregate, Column, Field, Meta};

use crate::events::PurchaseCompleted;
use crate::provider::PurchaseChange;

/// A one-time purchase, kept once per provider reference.
pub(crate) struct Purchase {
    meta: Meta,
    subscriber: Uuid,
    product: String,
    quantity: i64,
    provider: String,
    reference: String,
    purchased_at: Timestamp,
}

pub(crate) const REFERENCE: Column<Purchase, String> = Column::new("reference");

impl Purchase {
    pub(crate) fn completed(
        provider: &'static str,
        change: &PurchaseChange,
        product: &str,
    ) -> Result<Self> {
        let mut purchase = Self {
            meta: Meta::new()?,
            subscriber: change.subscriber.uuid(),
            product: product.to_owned(),
            quantity: i64::from(change.quantity),
            provider: provider.to_owned(),
            reference: change.reference.clone(),
            purchased_at: change.occurred_at,
        };
        purchase.meta.raise(PurchaseCompleted {
            subscriber: purchase.subscriber,
            product: purchase.product.clone(),
            quantity: change.quantity,
        })?;
        Ok(purchase)
    }
}

impl Aggregate for Purchase {
    const NAME: &'static str = "purchase";
    const MODULE: &'static str = "sofie_billing";
    const FIELDS: &'static [Field] = &[
        Field::new("subscriber", Uuid::COLUMNS, false),
        Field::new("product", String::COLUMNS, false),
        Field::new("quantity", i64::COLUMNS, false),
        Field::new("provider", String::COLUMNS, false),
        Field::new("reference", String::COLUMNS, false),
        Field::new("purchased_at", Timestamp::COLUMNS, false),
    ];
    const UNIQUE: &'static [&'static str] = &["reference"];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.subscriber.encode(out);
        self.product.encode(out);
        self.quantity.encode(out);
        self.provider.encode(out);
        self.reference.encode(out);
        self.purchased_at.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            subscriber: Uuid::decode(row)?,
            product: String::decode(row)?,
            quantity: i64::decode(row)?,
            provider: String::decode(row)?,
            reference: String::decode(row)?,
            purchased_at: Timestamp::decode(row)?,
        })
    }
}
