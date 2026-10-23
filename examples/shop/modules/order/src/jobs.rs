use std::time::Duration;

use sofie::prelude::*;

use crate::order::{Order, OrderStatus};

const PAYMENT_WINDOW: Duration = Duration::from_hours(24);

pub async fn unpaid_orders(read: Read, clock: Clock) -> Result<Vec<Order>> {
    let placed_before = clock.now().checked_sub(PAYMENT_WINDOW)?;
    Order::query()
        .where_(Order::STATUS.eq(OrderStatus::Pending))
        .where_(Order::PLACED_AT.lt(placed_before))
        .all(&read)
        .await
}

#[every("1h", each = unpaid_orders)]
pub async fn expire_unpaid(mut order: Order) -> Result<Order> {
    order.expire()?;
    Ok(order)
}
