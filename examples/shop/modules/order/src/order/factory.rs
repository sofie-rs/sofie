use sofie::prelude::*;
use sofie_identity::AccountId;

use super::{Order, OrderStatus};
use order_contract::OrderPlaced;

pub fn place(account: AccountId, email: Email, total: Money, placed_at: Instant) -> Result<Order> {
    if !total.is_positive() {
        return Err(Error::invalid("total", "must be positive"));
    }
    let mut order = Order {
        meta: Meta::new()?,
        account,
        email,
        status: OrderStatus::Pending,
        total,
        placed_at,
    };
    order.meta.raise(OrderPlaced {
        order: order.id().uuid(),
    })?;
    Ok(order)
}
