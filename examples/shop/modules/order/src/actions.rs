use order_contract::Customer;
use sofie::prelude::*;
use sofie_identity::Account;

use crate::order::{CancelReason, Order, factory};

#[derive(Input)]
pub struct PlaceOrder {
    total: Money,
}

/// The order is sent to the email the customer's account has now.
pub async fn place_order(
    customer: Principal<Customer>,
    input: PlaceOrder,
    read: Read,
    clock: Clock,
) -> Result<Order> {
    let account = Account::find(&read, customer.id()).await?;
    factory::place(
        customer.id(),
        account.email().clone(),
        input.total,
        clock.now(),
    )
}

pub async fn cancel_order(mut order: Order, cancel_reason: Option<CancelReason>) -> Result<Order> {
    order.cancel(cancel_reason)?;
    Ok(order)
}
