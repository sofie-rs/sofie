pub mod factory;

use sofie::prelude::*;

use crate::order::Order;

/// What happened to an order, recorded by the listeners as it happened.
#[derive(Aggregate)]
pub struct AuditEntry {
    meta: Meta,
    order: Id<Order>,
    action: AuditAction,
    recorded_at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enum)]
pub enum AuditAction {
    Placed,
    Cancelled,
}
