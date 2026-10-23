use order_contract::Customer;
use sofie::prelude::*;

use crate::{actions, queries};

pub fn routes(routes: &mut Routes<Customer>) {
    routes.screen("/orders", "orders", queries::list_orders);
    routes
        .post("/orders", actions::place_order)
        .then_screen("orders", "orders");
    routes
        .post("/orders/{order}/cancel", actions::cancel_order)
        .then_screen("orders", "orders");
}
