use serde::{Deserialize, Serialize};
use sofie_core::{Error, Result, Uuid};
use sofie_db::{Event, Input, Resolve, Writes};

use crate::Status;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionChanged {
    pub subscriber: Uuid,
    pub plan: String,
    pub status: Status,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseCompleted {
    pub subscriber: Uuid,
    pub product: String,
    pub quantity: u32,
}

/// Sent to the provider by a job after its commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SeatsAsked {
    pub(crate) subscriber: Uuid,
    pub(crate) seats: u32,
}

impl Event for SubscriptionChanged {
    const NAME: &'static str = "sofie_billing.SubscriptionChanged";
}

impl Event for PurchaseCompleted {
    const NAME: &'static str = "sofie_billing.PurchaseCompleted";
}

impl Event for SeatsAsked {
    const NAME: &'static str = "sofie_billing.SeatsAsked";
}

impl Input for SubscriptionChanged {
    fn from_json(value: serde_json::Value) -> Result<Self> {
        read(value, Self::NAME)
    }
}

impl Input for PurchaseCompleted {
    fn from_json(value: serde_json::Value) -> Result<Self> {
        read(value, Self::NAME)
    }
}

impl Input for SeatsAsked {
    fn from_json(value: serde_json::Value) -> Result<Self> {
        read(value, Self::NAME)
    }
}

impl<C: Writes> Resolve<C> for SubscriptionChanged {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(context.event(Self::NAME).and_then(Self::from_json))
    }
}

impl<C: Writes> Resolve<C> for SeatsAsked {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(context.event(Self::NAME).and_then(Self::from_json))
    }
}

fn read<T: for<'de> Deserialize<'de>>(value: serde_json::Value, name: &str) -> Result<T> {
    serde_json::from_value(value)
        .map_err(|error| Error::internal(format!("reading event `{name}`"), error))
}
