use serde::{Deserialize, Serialize};

use crate::models::orders::{self, OrderStatus};

#[derive(Debug, Deserialize, Serialize)]
pub struct Money {
    pub minor: i64,
    pub currency: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct OrderResponse {
    pub id: i64,
    pub status: String,
    pub total: Money,
    pub total_display: String,
    pub placed_at: String,
    pub can_cancel: bool,
}

impl From<&orders::Model> for OrderResponse {
    fn from(order: &orders::Model) -> Self {
        let can_cancel = order
            .status()
            .is_ok_and(|status| status.can_move_to(OrderStatus::Cancelled));
        Self {
            id: order.id,
            status: order.status.clone(),
            total: Money {
                minor: order.total,
                currency: order.currency.clone(),
            },
            total_display: format!(
                "{} {}",
                orders::format_major(order.total, &order.currency),
                order.currency
            ),
            placed_at: order.placed_at.to_rfc3339(),
            can_cancel,
        }
    }
}
