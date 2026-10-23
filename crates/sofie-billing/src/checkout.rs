use sofie_core::{Error, Result};
use sofie_db::{Context, Outcome, Reply, Resolve};

use crate::subscription::Subscription;
use crate::{Billing, Plan, Product, Subscriber};

/// Sends a subscriber to the provider's pages: to pay for a plan or product, or to manage
/// what it pays for.
#[derive(Clone)]
pub struct Checkout(Billing);

impl Checkout {
    pub async fn subscribe(
        &self,
        subscriber: Subscriber,
        plan: Plan,
        seats: u32,
        return_path: &str,
    ) -> Result<Link> {
        let plan = self.0.catalog.plans.declared(plan)?;
        let subscription = self.0.subscription(subscriber).await?;
        let running = subscription
            .as_ref()
            .is_some_and(|held| held.standing(self.0.now()).renews_at_provider());
        if running {
            return Err(Error::Rule(PLAN_RUNNING.to_owned()));
        }
        self.pay(
            subscriber,
            subscription,
            plan.name(),
            seats,
            true,
            return_path,
        )
        .await
    }

    pub async fn buy(
        &self,
        subscriber: Subscriber,
        product: Product,
        quantity: u32,
        return_path: &str,
    ) -> Result<Link> {
        let product = self.0.catalog.plans.declared_product(product)?;
        let subscription = self.0.subscription(subscriber).await?;
        self.pay(
            subscriber,
            subscription,
            product.name(),
            quantity,
            false,
            return_path,
        )
        .await
    }

    pub async fn portal(&self, subscriber: Subscriber, return_path: &str) -> Result<Link> {
        let subscription = self.0.subscription(subscriber).await?;
        let provider_customer = subscription
            .as_ref()
            .and_then(Subscription::provider_customer)
            .ok_or_else(|| Error::Rule("Nothing has been paid for yet.".to_owned()))?;
        self.0
            .catalog
            .setup
            .portal(provider_customer, return_path)
            .await
    }

    async fn pay(
        &self,
        subscriber: Subscriber,
        subscription: Option<Subscription>,
        name: &str,
        quantity: u32,
        recurring: bool,
        return_path: &str,
    ) -> Result<Link> {
        let provider_customer = subscription
            .as_ref()
            .and_then(Subscription::provider_customer);
        self.0
            .catalog
            .setup
            .checkout(
                subscriber,
                provider_customer,
                name,
                quantity,
                recurring,
                return_path,
            )
            .await
    }
}

const PLAN_RUNNING: &str = "A plan already renews; change it from the billing portal.";

impl<C: Context> Resolve<C> for Checkout {
    async fn resolve(context: &mut C) -> Result<Self> {
        Billing::resolve(context).await.map(Self)
    }
}

/// A link to a page of the provider's; return it from an action routed `.then_follow()`.
#[derive(Clone, Debug)]
pub struct Link {
    pub(crate) location: String,
}

impl Link {
    #[must_use]
    pub fn location(&self) -> &str {
        &self.location
    }
}

impl Outcome for Link {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(
            serde_json::json!({ "location": self.location }),
        ))
    }
}
