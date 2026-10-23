use std::sync::Arc;

use serde_json::Value;
use sofie_core::Result;
use sofie_db::{Outcome, Plan as CommitPlan, Reply, Resolve, Select, Stamp, Writes};
use sofie_http::{Delivery, Routes, Verify, Web};

use crate::Subscriber;
use crate::billing::{Billing, Catalog};
use crate::provider::{Change, Provider, PurchaseChange, SubscriptionChange};
use crate::purchase::{self, Purchase};
use crate::subscription::{Followed, Subscription};

pub const WEBHOOK_PATH: &str = "/webhooks/billing";

impl Billing {
    /// Without a provider there is nothing to hear from, so no route.
    pub fn routes(&self, routes: &mut Routes<Web>) {
        let Some(provider) = self.catalog.setup.provider.clone() else {
            return;
        };
        routes
            .webhook(WEBHOOK_PATH, receive)
            .verify(Signature(provider));
    }
}

struct Signature(Arc<dyn Provider>);

impl Verify for Signature {
    fn verify(&self, delivery: &Delivery<'_>) -> Result<String> {
        self.0.verify(delivery)
    }
}

pub(crate) struct Notification(Value);

impl<C: Writes> Resolve<C> for Notification {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(context.input().map(Self))
    }
}

pub(crate) enum Received {
    Nothing,
    Subscription(Subscription),
    Purchase(Purchase),
}

async fn receive(notification: Notification, billing: Billing) -> Result<Received> {
    let provider = billing.catalog.setup.provider()?;
    match provider.read(&notification.0)? {
        None => Ok(Received::Nothing),
        Some(Change::Subscription(change)) => {
            subscription_changed(&billing, provider.name(), &change).await
        }
        Some(Change::Purchase(change)) => purchased(&billing, provider.name(), &change).await,
    }
}

async fn subscription_changed(
    billing: &Billing,
    provider: &'static str,
    change: &SubscriptionChange,
) -> Result<Received> {
    let Some(status) = change.status else {
        return Ok(Received::Nothing);
    };
    let plan = plan_of(&billing.catalog, &change.price);
    let now = billing.now();
    let Some(mut subscription) = billing.subscription(change.subscriber).await? else {
        if status == crate::Status::Canceled {
            return Ok(Received::Nothing);
        }
        let plan = match plan {
            Ok(plan) => plan,
            Err(error) => {
                unpriced(change.subscriber, &change.price, &error, "granted nothing");
                return Ok(Received::Nothing);
            }
        };
        let started = Subscription::from_provider(provider, change, status, plan, now)?;
        return Ok(Received::Subscription(started));
    };
    let plan = match plan {
        Ok(plan) => plan.to_owned(),
        // The held plan was paid for under the subscription held, not under another one.
        Err(error) if subscription.is_held(change) => {
            let outcome = "kept the held plan";
            unpriced(change.subscriber, &change.price, &error, outcome);
            subscription.plan_name().to_owned()
        }
        Err(error) => {
            unpriced(change.subscriber, &change.price, &error, "granted nothing");
            return Ok(Received::Nothing);
        }
    };
    match subscription.follow(provider, change, status, &plan, now)? {
        Followed::Changed => Ok(Received::Subscription(subscription)),
        Followed::Ignored(why) => {
            tracing::info!(
                subscriber = %change.subscriber.uuid(),
                status = status.as_str(),
                why,
                "ignored a provider's subscription change"
            );
            Ok(Received::Nothing)
        }
    }
}

/// A provider may report one checkout twice under different events.
async fn purchased(
    billing: &Billing,
    provider: &'static str,
    change: &PurchaseChange,
) -> Result<Received> {
    let product = match product_of(&billing.catalog, &change.price) {
        Ok(product) => product,
        Err(error) => {
            unpriced(change.subscriber, &change.price, &error, "kept no purchase");
            return Ok(Received::Nothing);
        }
    };
    let known = Purchase::query()
        .where_(purchase::REFERENCE.eq(change.reference.clone()))
        .exists(&billing.db.read())
        .await?;
    if known {
        return Ok(Received::Nothing);
    }
    Purchase::completed(provider, change, product).map(Received::Purchase)
}

/// Refusing would have the provider retry until it gives up, and refuse every later change
/// of the subscription too, its cancellation among them; only the operator can fix
/// `SOFIE_BILLING_PRICES`, so it is logged as an error.
fn unpriced(subscriber: Subscriber, price: &str, error: &sofie_core::Error, outcome: &'static str) {
    tracing::error!(
        subscriber = %subscriber.uuid(),
        price,
        %error,
        outcome,
        "acknowledged a provider's change of a price SOFIE_BILLING_PRICES does not name"
    );
}

fn product_of<'c>(catalog: &'c Catalog, price: &str) -> Result<&'c str> {
    let name = catalog.setup.name_of(price)?;
    catalog
        .plans
        .product(name)
        .map(crate::Product::name)
        .ok_or_else(|| {
            sofie_core::Error::internal_message(format!(
                "price `{price}` names `{name}`, which is not a product"
            ))
        })
}

fn plan_of<'c>(catalog: &'c Catalog, price: &str) -> Result<&'c str> {
    let name = catalog.setup.name_of(price)?;
    catalog
        .plans
        .plan(name)
        .map(crate::Plan::name)
        .ok_or_else(|| {
            sofie_core::Error::internal_message(format!(
                "price `{price}` names `{name}`, which is not a plan"
            ))
        })
}

impl Outcome for Received {
    fn plan(&self, plan: &mut CommitPlan<'_>) -> Result<()> {
        match self {
            Self::Nothing => Ok(()),
            Self::Subscription(subscription) => subscription.plan(plan),
            Self::Purchase(purchase) => purchase.plan(plan),
        }
    }

    fn committed(&mut self, stamp: Stamp) {
        match self {
            Self::Nothing => {}
            Self::Subscription(subscription) => subscription.committed(stamp),
            Self::Purchase(purchase) => purchase.committed(stamp),
        }
    }

    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(serde_json::json!({ "received": true })))
    }
}
