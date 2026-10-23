use sofie::prelude::*;

use crate::order::{Order, OrderStatus};

#[derive(ReadModel)]
#[read(from = Order)]
pub struct OrderRow {
    id: Id<Order>,
    status: OrderStatus,
    total: Money,
}

pub async fn list_orders(read: Read) -> Result<Vec<OrderRow>> {
    OrderRow::query().latest().limit(50).all(&read).await
}
