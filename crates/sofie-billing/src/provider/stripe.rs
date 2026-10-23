use serde_json::Value;
use sofie_client::Outgoing;
use sofie_core::{Error, Result, Timestamp};
use sofie_db::{BoxFuture, IdempotencyKey};
use sofie_http::Delivery;

use super::signature::Scheme;
use super::{
    Cancellation, Change, Checkout, Provider, ProviderKeys, PurchaseChange, SeatsChange,
    SubscriptionChange, field, missing, optional_text, path_segment, quantity, subscriber, text,
};
use crate::Status;

/// `Stripe-Signature: t=<seconds>,v1=<hex>[,v1=<hex>]`.
const SCHEME: Scheme = Scheme {
    header: "stripe-signature",
    parts: ',',
    timestamp: "t",
    signature: "v1",
    separator: b'.',
};

pub(crate) struct Stripe {
    http: Outgoing,
    keys: ProviderKeys,
}

impl Stripe {
    pub(crate) fn new(http: Outgoing, keys: ProviderKeys) -> Self {
        Self { http, keys }
    }

    async fn create_checkout(&self, checkout: &Checkout<'_>) -> Result<String> {
        let subscriber = checkout.subscriber.uuid().to_string();
        let quantity = checkout.quantity.to_string();
        let mut fields = vec![
            (
                "mode",
                if checkout.recurring {
                    "subscription"
                } else {
                    "payment"
                },
            ),
            ("line_items[0][price]", checkout.price),
            ("line_items[0][quantity]", quantity.as_str()),
            ("success_url", checkout.return_url),
            ("cancel_url", checkout.return_url),
            ("client_reference_id", subscriber.as_str()),
            ("metadata[subscriber]", subscriber.as_str()),
            ("metadata[price]", checkout.price),
            ("metadata[quantity]", quantity.as_str()),
        ];
        if checkout.recurring {
            fields.push((
                "subscription_data[metadata][subscriber]",
                subscriber.as_str(),
            ));
        }
        if let Some(provider_customer) = checkout.provider_customer {
            fields.push(("customer", provider_customer));
        }
        self.session("/v1/checkout/sessions", &fields).await
    }

    async fn update_seats(&self, change: &SeatsChange<'_>) -> Result<()> {
        let id = path_segment(change.subscription, "subscription")?;
        let subscription: Value = self
            .http
            .get(&format!("/v1/subscriptions/{id}"))
            .bearer(&self.keys.api_key)
            .send()
            .await?
            .json()?;
        let item = path_segment(
            text(&subscription, "/items/data/0/id")?,
            "subscription item",
        )?;
        let quantity = change.seats.to_string();
        let _: Value = self
            .http
            .post(&format!("/v1/subscription_items/{item}"))
            .bearer(&self.keys.api_key)
            .idempotency(change.key)
            .form(&[
                ("quantity", quantity.as_str()),
                ("proration_behavior", "create_prorations"),
            ])
            .send()
            .await?
            .json()?;
        Ok(())
    }

    async fn cancel_now(&self, cancellation: &Cancellation<'_>) -> Result<()> {
        let id = path_segment(cancellation.subscription, "subscription")?;
        let _: Value = self
            .http
            .delete(&format!("/v1/subscriptions/{id}"))
            .bearer(&self.keys.api_key)
            .idempotency(cancellation.key)
            .send()
            .await?
            .json()?;
        Ok(())
    }

    async fn session(&self, path: &str, fields: &[(&str, &str)]) -> Result<String> {
        let session: Value = self
            .http
            .post(path)
            .bearer(&self.keys.api_key)
            .idempotency(&IdempotencyKey::generate()?)
            .form(fields)
            .send()
            .await?
            .json()?;
        text(&session, "/url").map(str::to_owned)
    }
}

impl Provider for Stripe {
    fn name(&self) -> &'static str {
        "stripe"
    }

    fn checkout<'a>(&'a self, checkout: &'a Checkout<'a>) -> BoxFuture<'a, Result<String>> {
        Box::pin(self.create_checkout(checkout))
    }

    fn portal<'a>(
        &'a self,
        provider_customer: &'a str,
        return_url: &'a str,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let fields = [("customer", provider_customer), ("return_url", return_url)];
            self.session("/v1/billing_portal/sessions", &fields).await
        })
    }

    fn verify(&self, delivery: &Delivery<'_>) -> Result<String> {
        let event = SCHEME.verify(delivery, &self.keys.webhook_secret)?;
        text(&event, "/id").map(str::to_owned)
    }

    fn change_seats<'a>(&'a self, change: &'a SeatsChange<'a>) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.update_seats(change))
    }

    fn cancel<'a>(&'a self, cancellation: &'a Cancellation<'a>) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.cancel_now(cancellation))
    }

    fn read(&self, event: &Value) -> Result<Option<Change>> {
        let occurred_at = seconds(event.pointer("/created"))?.ok_or_else(|| missing("/created"))?;
        let object = field(event, "/data/object")?;
        match text(event, "/type")? {
            "customer.subscription.created"
            | "customer.subscription.updated"
            | "customer.subscription.deleted"
            | "customer.subscription.paused"
            | "customer.subscription.resumed" => subscription(object, occurred_at),
            "checkout.session.completed" | "checkout.session.async_payment_succeeded" => {
                purchase(object, occurred_at)
            }
            _ => Ok(None),
        }
    }
}

fn subscription(object: &Value, occurred_at: Timestamp) -> Result<Option<Change>> {
    let Some(subscriber) = subscriber(optional_text(object, "/metadata/subscriber"))? else {
        return Ok(None);
    };
    let item = field(object, "/items/data/0")?;
    // Newer API versions keep the period on the item rather than the subscription.
    let period = |field: &str| -> Result<Option<Timestamp>> {
        match seconds(object.get(field))? {
            Some(at) => Ok(Some(at)),
            None => seconds(item.get(field)),
        }
    };
    Ok(Some(Change::Subscription(SubscriptionChange {
        subscriber,
        subscription: text(object, "/id")?.to_owned(),
        provider_customer: text(object, "/customer")?.to_owned(),
        price: text(item, "/price/id")?.to_owned(),
        status: status(text(object, "/status")?),
        quantity: quantity(item.get("quantity"))?,
        period_start: period("current_period_start")?.unwrap_or(occurred_at),
        period_ends_at: period("current_period_end")?,
        trial_ends_at: seconds(object.get("trial_end"))?,
        cancels_at_period_end: object
            .get("cancel_at_period_end")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        occurred_at,
    })))
}

/// A subscription's checkout is followed through its subscription's events instead.
fn purchase(session: &Value, occurred_at: Timestamp) -> Result<Option<Change>> {
    let paid = optional_text(session, "/payment_status") == Some("paid");
    if optional_text(session, "/mode") != Some("payment") || !paid {
        return Ok(None);
    }
    let Some(subscriber) = subscriber(optional_text(session, "/client_reference_id"))? else {
        return Ok(None);
    };
    let quantity = optional_text(session, "/metadata/quantity")
        .map(|text| {
            text.parse()
                .map_err(|_| Error::BadRequest("the checkout's quantity is not a count".to_owned()))
        })
        .transpose()?
        .unwrap_or(1);
    Ok(Some(Change::Purchase(PurchaseChange {
        subscriber,
        reference: text(session, "/id")?.to_owned(),
        price: text(session, "/metadata/price")?.to_owned(),
        quantity,
        occurred_at,
    })))
}

/// `incomplete` waits for a first payment the owner has yet to confirm.
fn status(stripe: &str) -> Option<Status> {
    match stripe {
        "trialing" => Some(Status::Trialing),
        "active" => Some(Status::Active),
        "past_due" | "unpaid" => Some(Status::PastDue),
        "canceled" | "incomplete_expired" | "paused" => Some(Status::Canceled),
        _ => None,
    }
}

fn seconds(value: Option<&Value>) -> Result<Option<Timestamp>> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    value
        .as_i64()
        .and_then(|seconds| Timestamp::from_second(seconds).ok())
        .map(Some)
        .ok_or_else(|| Error::BadRequest("a time in the event is not Unix seconds".to_owned()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const SUBSCRIBER: &str = "0192d3a4-0000-7000-8000-000000000001";

    fn read(event: &Value) -> Option<Change> {
        let http = sofie_client::for_vendor("stripe", sofie_client::Vendor::default()).unwrap();
        let keys = ProviderKeys {
            api_key: "sk_test".to_owned(),
            webhook_secret: "whsec_test".to_owned(),
        };
        Stripe::new(http, keys).read(event).unwrap()
    }

    #[test]
    fn a_subscription_event_names_its_owner_price_status_and_period() {
        let event = json!({
            "id": "evt_1", "type": "customer.subscription.updated", "created": 1_760_000_100,
            "data": {"object": {
                "id": "sub_1", "customer": "cus_1", "status": "past_due",
                "cancel_at_period_end": true, "trial_end": null,
                "metadata": {"subscriber": SUBSCRIBER},
                "items": {"data": [{
                    "price": {"id": "price_pro"}, "quantity": 3,
                    "current_period_start": 1_760_000_000, "current_period_end": 1_762_592_000
                }]}
            }}
        });

        let Some(Change::Subscription(change)) = read(&event) else {
            panic!("a subscription change");
        };

        assert_eq!(change.subscriber.uuid().to_string(), SUBSCRIBER);
        assert_eq!(change.price, "price_pro");
        assert_eq!(change.status, Some(Status::PastDue));
        assert_eq!(change.quantity, 3);
        assert_eq!(change.period_start.as_second(), 1_760_000_000);
        assert!(change.cancels_at_period_end);
    }

    #[test]
    fn a_paid_one_time_checkout_is_a_purchase_and_others_are_not_followed() {
        let session = |mode: &str| {
            json!({
                "id": "evt_2", "type": "checkout.session.completed", "created": 1_760_000_100,
                "data": {"object": {
                    "id": "cs_1", "mode": mode, "payment_status": "paid",
                    "client_reference_id": SUBSCRIBER,
                    "metadata": {"price": "price_credits", "quantity": "2"}
                }}
            })
        };

        let Some(Change::Purchase(purchase)) = read(&session("payment")) else {
            panic!("a purchase");
        };

        assert_eq!(
            (purchase.reference.as_str(), purchase.quantity),
            ("cs_1", 2)
        );
        assert!(read(&session("subscription")).is_none());
        assert!(
            read(&json!({"type": "invoice.paid", "created": 1, "data": {"object": {}}})).is_none()
        );
    }
}
