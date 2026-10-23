use sofie::prelude::*;

use crate::audit::{AuditAction, AuditEntry, factory};
use order_contract::{OrderCancelled, OrderPlaced};

#[on]
pub async fn audit_placement(event: OrderPlaced, clock: Clock) -> Result<AuditEntry> {
    factory::record(Id::from_uuid(event.order), AuditAction::Placed, clock.now())
}

#[on]
pub async fn audit_cancellation(event: OrderCancelled, clock: Clock) -> Result<AuditEntry> {
    factory::record(
        Id::from_uuid(event.order),
        AuditAction::Cancelled,
        clock.now(),
    )
}
