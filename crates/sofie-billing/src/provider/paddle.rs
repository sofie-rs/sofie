use serde_json::{Value, json};
use sofie_client::Outgoing;
use sofie_core::{Error, Result, Timestamp};
use sofie_db::BoxFuture;
use sofie_http::Delivery;

use super::signature::Scheme;
use super::{
    Cancellation, Change, Checkout, Provider, ProviderKeys, PurchaseChange, SeatsChange,
    SubscriptionChange, field, missing, optional_text, path_segment, quantity, subscriber, text,
};
use crate::Status;

/// `Paddle-Signature: ts=<seconds>;h1=<hex>[;h1=<hex>]`.
const SCHEME: Scheme = Scheme {
    header: "paddle-signature",
    parts: ';',
    timestamp: "ts",
    signature: "h1",
    separator: b':',
};

pub(crate) struct Paddle {
    http: Outgoing,
    keys: ProviderKeys,
}

impl Paddle {
    pub(crate) fn new(http: Outgoing, keys: ProviderKeys) -> Self {
        Self { http, keys }
    }

    /// Where the owner lands after paying is set on Paddle's checkout, not per transaction.
    ///
    /// Unlike Stripe's, Paddle's POSTs carry no idempotency key and so are never retried: Paddle
    /// takes no client-supplied key (<https://developer.paddle.com/sdks/libraries/>).
    async fn create_checkout(&self, checkout: &Checkout<'_>) -> Result<String> {
        let mut transaction = json!({
            "items": [{"price_id": checkout.price, "quantity": checkout.quantity}],
            "custom_data": {"subscriber": checkout.subscriber.uuid().to_string()},
        });
        if let Some(provider_customer) = checkout.provider_customer {
            transaction["customer_id"] = json!(provider_customer);
        }
        let created: Value = self
            .http
            .post("/transactions")
            .bearer(&self.keys.api_key)
            .json(&transaction)
            .send()
            .await?
            .json()?;
        text(&created, "/data/checkout/url").map(str::to_owned)
    }

    async fn create_portal(&self, provider_customer: &str) -> Result<String> {
        let provider_customer = path_segment(provider_customer, "customer")?;
        let session: Value = self
            .http
            .post(&format!("/customers/{provider_customer}/portal-sessions"))
            .bearer(&self.keys.api_key)
            .json(&json!({}))
            .send()
            .await?
            .json()?;
        text(&session, "/data/urls/general/overview").map(str::to_owned)
    }

    /// Paddle replaces the items with those sent, so the held price goes back with the new count.
    async fn update_seats(&self, change: &SeatsChange<'_>) -> Result<()> {
        let path = format!(
            "/subscriptions/{}",
            path_segment(change.subscription, "subscription")?
        );
        let subscription: Value = self
            .http
            .get(&path)
            .bearer(&self.keys.api_key)
            .send()
            .await?
            .json()?;
        let update = json!({
            "items": [{
                "price_id": text(&subscription, "/data/items/0/price/id")?,
                "quantity": change.seats,
            }],
            "proration_billing_mode": "prorated_next_billing_period",
        });
        // A count, not an addition: sent again, it changes nothing more.
        let _: Value = self
            .http
            .patch(&path)
            .bearer(&self.keys.api_key)
            .json(&update)
            .repeatable()
            .send()
            .await?
            .json()?;
        Ok(())
    }

    async fn cancel_now(&self, cancellation: &Cancellation<'_>) -> Result<()> {
        let id = path_segment(cancellation.subscription, "subscription")?;
        // Canceled once, sent again it cancels nothing more.
        let _: Value = self
            .http
            .post(&format!("/subscriptions/{id}/cancel"))
            .bearer(&self.keys.api_key)
            .json(&json!({"effective_from": "immediately"}))
            .repeatable()
            .send()
            .await?
            .json()?;
        Ok(())
    }
}

impl Provider for Paddle {
    fn name(&self) -> &'static str {
        "paddle"
    }

    fn checkout<'a>(&'a self, checkout: &'a Checkout<'a>) -> BoxFuture<'a, Result<String>> {
        Box::pin(self.create_checkout(checkout))
    }

    /// Paddle's portal links back by its own settings, so the return URL goes unused.
    fn portal<'a>(
        &'a self,
        provider_customer: &'a str,
        _return_url: &'a str,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(self.create_portal(provider_customer))
    }

    fn verify(&self, delivery: &Delivery<'_>) -> Result<String> {
        let notification = SCHEME.verify(delivery, &self.keys.webhook_secret)?;
        text(&notification, "/event_id").map(str::to_owned)
    }

    fn change_seats<'a>(&'a self, change: &'a SeatsChange<'a>) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.update_seats(change))
    }

    fn cancel<'a>(&'a self, cancellation: &'a Cancellation<'a>) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.cancel_now(cancellation))
    }

    fn read(&self, notification: &Value) -> Result<Option<Change>> {
        let occurred_at =
            instant(notification, "/occurred_at")?.ok_or_else(|| missing("/occurred_at"))?;
        let data = field(notification, "/data")?;
        let kind = text(notification, "/event_type")?;
        if kind.starts_with("subscription.") {
            return subscription(data, occurred_at);
        }
        if kind == "transaction.completed" {
            return purchase(data, occurred_at);
        }
        Ok(None)
    }
}

fn subscription(data: &Value, occurred_at: Timestamp) -> Result<Option<Change>> {
    let Some(subscriber) = subscriber(optional_text(data, "/custom_data/subscriber"))? else {
        return Ok(None);
    };
    let item = field(data, "/items/0")?;
    let scheduled = optional_text(data, "/scheduled_change/action");
    Ok(Some(Change::Subscription(SubscriptionChange {
        subscriber,
        subscription: text(data, "/id")?.to_owned(),
        provider_customer: text(data, "/customer_id")?.to_owned(),
        price: text(item, "/price/id")?.to_owned(),
        status: status(text(data, "/status")?),
        quantity: quantity(item.get("quantity"))?,
        period_start: instant(data, "/current_billing_period/starts_at")?.unwrap_or(occurred_at),
        period_ends_at: instant(data, "/current_billing_period/ends_at")?,
        trial_ends_at: instant(item, "/trial_dates/ends_at")?,
        cancels_at_period_end: scheduled == Some("cancel"),
        occurred_at,
    })))
}

/// A transaction of a subscription is followed through the subscription's notifications instead.
fn purchase(data: &Value, occurred_at: Timestamp) -> Result<Option<Change>> {
    if data.get("subscription_id").is_some_and(|id| !id.is_null()) {
        return Ok(None);
    }
    let Some(subscriber) = subscriber(optional_text(data, "/custom_data/subscriber"))? else {
        return Ok(None);
    };
    let item = field(data, "/items/0")?;
    Ok(Some(Change::Purchase(PurchaseChange {
        subscriber,
        reference: text(data, "/id")?.to_owned(),
        price: text(item, "/price/id")?.to_owned(),
        quantity: quantity(item.get("quantity"))?,
        occurred_at,
    })))
}

fn status(paddle: &str) -> Option<Status> {
    match paddle {
        "trialing" => Some(Status::Trialing),
        "active" => Some(Status::Active),
        "past_due" => Some(Status::PastDue),
        "canceled" | "paused" => Some(Status::Canceled),
        _ => None,
    }
}

fn instant(value: &Value, pointer: &str) -> Result<Option<Timestamp>> {
    optional_text(value, pointer)
        .map(|text| {
            text.parse()
                .map_err(|_| Error::BadRequest(format!("`{pointer}` is not an RFC 3339 time")))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUBSCRIBER: &str = "0192d3a4-0000-7000-8000-000000000002";

    fn read(notification: &Value) -> Option<Change> {
        let http = sofie_client::for_vendor("paddle", sofie_client::Vendor::default()).unwrap();
        let keys = ProviderKeys {
            api_key: "pdl_test".to_owned(),
            webhook_secret: "pdl_ntfset_test".to_owned(),
        };
        Paddle::new(http, keys).read(notification).unwrap()
    }

    #[test]
    fn a_subscription_notification_names_its_owner_and_a_scheduled_cancel() {
        let notification = json!({
            "event_id": "evt_01", "event_type": "subscription.updated",
            "occurred_at": "2026-10-14T10:00:00.000Z",
            "data": {
                "id": "sub_01", "status": "trialing", "customer_id": "ctm_01",
                "custom_data": {"subscriber": SUBSCRIBER},
                "scheduled_change": {"action": "cancel", "effective_at": "2026-11-14T10:00:00Z"},
                "current_billing_period": {"starts_at": "2026-10-14T10:00:00Z", "ends_at": "2026-11-14T10:00:00Z"},
                "items": [{"price": {"id": "pri_pro"}, "quantity": 1,
                           "trial_dates": {"starts_at": "2026-10-14T10:00:00Z", "ends_at": "2026-10-28T10:00:00Z"}}]
            }
        });

        let Some(Change::Subscription(change)) = read(&notification) else {
            panic!("a subscription change");
        };

        assert_eq!(change.status, Some(Status::Trialing));
        assert_eq!(change.price, "pri_pro");
        assert!(change.cancels_at_period_end);
        assert_eq!(
            change.trial_ends_at,
            Some("2026-10-28T10:00:00Z".parse().unwrap())
        );
    }

    #[test]
    fn a_completed_one_time_transaction_is_a_purchase() {
        let transaction = |subscription: Value| {
            json!({
                "event_id": "evt_02", "event_type": "transaction.completed",
                "occurred_at": "2026-10-14T10:00:00Z",
                "data": {
                    "id": "txn_01", "subscription_id": subscription,
                    "custom_data": {"subscriber": SUBSCRIBER},
                    "items": [{"price": {"id": "pri_credits"}, "quantity": 5}]
                }
            })
        };

        let Some(Change::Purchase(purchase)) = read(&transaction(Value::Null)) else {
            panic!("a purchase");
        };

        assert_eq!(
            (purchase.reference.as_str(), purchase.quantity),
            ("txn_01", 5)
        );
        assert!(read(&transaction(json!("sub_01"))).is_none());
    }
}
