use sofie::prelude::*;

use super::{AuditAction, AuditEntry};
use crate::order::Order;

pub fn record(order: Id<Order>, action: AuditAction, recorded_at: Instant) -> Result<AuditEntry> {
    Ok(AuditEntry {
        meta: Meta::new()?,
        order,
        action,
        recorded_at,
    })
}
