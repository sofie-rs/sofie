pub mod factory;

use sofie::prelude::*;
use sofie_identity::AccountId;

use order_contract::OrderCancelled;

#[derive(Aggregate)]
pub struct Order {
    meta: Meta,
    #[owner]
    account: AccountId,
    email: Email,
    status: OrderStatus,
    total: Money,
    placed_at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enum)]
pub enum OrderStatus {
    #[to(Paid, Cancelled, Expired)]
    Pending,
    #[to(Shipped, Refunded)]
    Paid,
    Shipped,
    Cancelled,
    Refunded,
    Expired,
}

#[derive(Value)]
pub struct CancelReason(String);

impl CancelReason {
    fn try_new(text: String) -> Result<Self, &'static str> {
        let fits = text.chars().count() <= 200;
        fits.then_some(Self(text)).ok_or("over 200 characters")
    }
}

impl Order {
    pub fn cancel(&mut self, reason: Option<CancelReason>) -> Result<()> {
        self.status = self.status.to(OrderStatus::Cancelled)?;
        self.meta.raise(OrderCancelled {
            order: self.id().uuid(),
            reason: reason.map(|reason| reason.0),
        })
    }

    pub fn expire(&mut self) -> Result<()> {
        self.status = self.status.to(OrderStatus::Expired)?;
        Ok(())
    }
}
