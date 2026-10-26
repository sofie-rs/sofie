mod paddle;
mod signature;
mod stripe;

use serde_json::Value;
use sofie_client::Outgoing;
use sofie_core::{Error, Result, Timestamp, Uuid};
use sofie_db::{BoxFuture, IdempotencyKey};
use sofie_http::Delivery;

use crate::{Status, Subscriber};

pub(crate) use paddle::Paddle;
pub(crate) use stripe::Stripe;

pub(crate) trait Provider: Send + Sync + 'static {
    fn name(&self) -> &'static str;

    fn checkout<'a>(&'a self, checkout: &'a Checkout<'a>) -> BoxFuture<'a, Result<String>>;

    fn portal<'a>(
        &'a self,
        provider_customer: &'a str,
        return_url: &'a str,
    ) -> BoxFuture<'a, Result<String>>;

    /// The delivery's id once its signature holds.
    fn verify(&self, delivery: &Delivery<'_>) -> Result<String>;

    /// `None` for a notification billing does not follow.
    fn read(&self, notification: &Value) -> Result<Option<Change>>;

    /// Prorated; the subscription's notification brings the new count back.
    fn change_seats<'a>(&'a self, change: &'a SeatsChange<'a>) -> BoxFuture<'a, Result<()>>;

    /// At once rather than at the period's end; the subscription's notification follows.
    fn cancel<'a>(&'a self, cancellation: &'a Cancellation<'a>) -> BoxFuture<'a, Result<()>>;
}

pub(crate) struct Cancellation<'a> {
    pub(crate) subscription: &'a str,
    pub(crate) key: &'a IdempotencyKey,
}

pub(crate) struct SeatsChange<'a> {
    pub(crate) subscription: &'a str,
    pub(crate) seats: u32,
    pub(crate) key: &'a IdempotencyKey,
}

pub(crate) struct Checkout<'a> {
    pub(crate) subscriber: Subscriber,
    pub(crate) price: &'a str,
    pub(crate) quantity: u32,
    pub(crate) recurring: bool,
    pub(crate) provider_customer: Option<&'a str>,
    pub(crate) return_url: &'a str,
}

pub(crate) enum Change {
    Subscription(SubscriptionChange),
    Purchase(PurchaseChange),
}

pub(crate) struct SubscriptionChange {
    pub(crate) subscriber: Subscriber,
    pub(crate) subscription: String,
    pub(crate) provider_customer: String,
    pub(crate) price: String,
    /// `None` while the provider has not settled it, such as a first payment awaiting the owner.
    pub(crate) status: Option<Status>,
    pub(crate) quantity: u32,
    pub(crate) period_start: Timestamp,
    pub(crate) period_ends_at: Option<Timestamp>,
    pub(crate) trial_ends_at: Option<Timestamp>,
    pub(crate) cancels_at_period_end: bool,
    /// When the provider's subscription began, which tells two live ones of a subscriber apart.
    pub(crate) started_at: Option<Timestamp>,
    pub(crate) occurred_at: Timestamp,
}

pub(crate) struct PurchaseChange {
    pub(crate) subscriber: Subscriber,
    pub(crate) reference: String,
    pub(crate) price: String,
    pub(crate) quantity: u32,
    pub(crate) occurred_at: Timestamp,
}

/// What every provider is set up with; read from `SOFIE_BILLING_*`.
pub struct ProviderKeys {
    pub api_key: String,
    pub webhook_secret: String,
}

/// The providers billing speaks to, chosen by `SOFIE_BILLING_PROVIDER`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vendor {
    Stripe,
    /// Paddle Billing, a merchant of record: it sells in its own name and settles taxes.
    Paddle,
}

impl Vendor {
    pub(crate) fn parse(text: &str) -> Result<Self> {
        match text {
            "stripe" => Ok(Self::Stripe),
            "paddle" => Ok(Self::Paddle),
            other => Err(Error::internal_message(format!(
                "SOFIE_BILLING_PROVIDER is `{other}`, not one of stripe, paddle"
            ))),
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Stripe => "stripe",
            Self::Paddle => "paddle",
        }
    }

    #[must_use]
    pub const fn api_url(self) -> &'static str {
        match self {
            Self::Stripe => "https://api.stripe.com",
            Self::Paddle => "https://api.paddle.com",
        }
    }

    pub(crate) fn build(self, http: Outgoing, keys: ProviderKeys) -> Box<dyn Provider> {
        match self {
            Self::Stripe => Box::new(Stripe::new(http, keys)),
            Self::Paddle => Box::new(Paddle::new(http, keys)),
        }
    }
}

/// A field the provider must send, named in the error without its value.
pub(crate) fn text<'v>(value: &'v Value, pointer: &str) -> Result<&'v str> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| missing(pointer))
}

pub(crate) fn field<'v>(value: &'v Value, pointer: &str) -> Result<&'v Value> {
    value.pointer(pointer).ok_or_else(|| missing(pointer))
}

pub(crate) fn optional_text<'v>(value: &'v Value, pointer: &str) -> Option<&'v str> {
    value.pointer(pointer).and_then(Value::as_str)
}

/// An id the provider gave, checked before it goes into a request path.
pub(crate) fn path_segment<'v>(id: &'v str, what: &str) -> Result<&'v str> {
    if !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Ok(id);
    }
    Err(Error::internal_message(format!(
        "a {what} id from the provider is malformed"
    )))
}

pub(crate) fn missing(pointer: &str) -> Error {
    Error::BadRequest(format!("the notification has no `{pointer}`"))
}

pub(crate) fn quantity(value: Option<&Value>) -> Result<u32> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(1);
    };
    value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .ok_or_else(|| Error::BadRequest("the notification's quantity is not a count".to_owned()))
}

/// Billing names the owner on every checkout, so a change it did not start names none.
pub(crate) fn subscriber(text: Option<&str>) -> Result<Option<Subscriber>> {
    text.map(|text| {
        text.parse::<Uuid>()
            .map(Subscriber::new)
            .map_err(|_| Error::BadRequest("the notification's subscriber is not an id".to_owned()))
    })
    .transpose()
}
