use std::sync::Arc;

use sofie_core::{Error, Result, Timestamp, Uuid};
use sofie_db::{
    BoxFuture, Context, Db, IdempotencyKey, Outcome, Plan as CommitPlan, Read, Reply, Resolve,
    Stamp,
};
use sofie_http::{Area, Entitled, Principal, RequestContext};

use crate::entitlements::Entitlements;
use crate::provider::Cancellation;
use crate::subscription::Subscription;
use crate::{Plan, Plans, Product, SeatRule, Seats, Setup};

/// Whoever a subscription and its quotas belong to: an account, or an organization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Subscriber(Uuid);

impl Subscriber {
    #[must_use]
    pub const fn new(uuid: Uuid) -> Self {
        Self(uuid)
    }

    #[must_use]
    pub const fn uuid(self) -> Uuid {
        self.0
    }
}

impl<T> From<sofie_db::Id<T>> for Subscriber {
    fn from(id: sofie_db::Id<T>) -> Self {
        Self(id.uuid())
    }
}

/// What `install` provides to the database; holds no `Db`, which provides it.
pub(crate) struct Catalog {
    pub(crate) plans: &'static Plans,
    pub(crate) setup: Setup,
}

#[derive(Clone)]
pub struct Billing {
    pub(crate) catalog: Arc<Catalog>,
    pub(crate) db: Db,
}

/// The app's plans on `db`, with what `configure` set up there or no provider.
pub fn install(db: &Db, plans: &'static Plans) -> Billing {
    let setup = sofie_db::provided::<Setup>(db)
        .map_or_else(Setup::without_provider, |setup| Setup::clone(&setup));
    let catalog = Arc::new(Catalog { plans, setup });
    sofie_db::provide(db, Arc::clone(&catalog));
    Billing {
        catalog,
        db: db.clone(),
    }
}

impl Billing {
    /// Every priced name is a plan or product, so a checkout never names an unknown one.
    pub fn checked(self) -> Result<Self> {
        self.catalog.setup.check_prices(self.catalog.plans)?;
        Ok(self)
    }

    pub async fn entitlements(&self, subscriber: Subscriber) -> Result<Entitlements> {
        let subscription = self.subscription(subscriber).await?;
        Ok(Entitlements::of(
            self.clone(),
            subscriber,
            subscription.as_ref(),
        ))
    }

    pub async fn seats(&self, subscriber: Subscriber) -> Result<Seats> {
        Ok(self.entitlements(subscriber).await?.seats())
    }

    /// Billed at the provider by a job after the commit; `seats` follows once the provider confirms.
    pub async fn change_seats(
        &self,
        subscriber: Subscriber,
        seats: u32,
    ) -> Result<SeatsChangeAsked> {
        if seats == 0 {
            return Err(Error::invalid("seats", "must be at least 1"));
        }
        self.catalog.setup.provider()?;
        let now = self.now();
        let mut subscription = self
            .subscription(subscriber)
            .await?
            .ok_or_else(|| Error::Rule("Nothing has been paid for yet.".to_owned()))?;
        let per_seat = subscription
            .plan_held(now)
            .and_then(|name| self.catalog.plans.plan(name))
            .is_some_and(|plan| plan.seat_rule() == SeatRule::Purchased);
        if !per_seat {
            return Err(Error::Rule(
                "The plan held is not billed per seat.".to_owned(),
            ));
        }
        subscription.ask_seats(seats, now)?;
        Ok(SeatsChangeAsked(subscription))
    }

    /// Without a card; once per owner, and not while a subscription runs.
    pub async fn start_trial(&self, subscriber: Subscriber, plan: Plan) -> Result<TrialStarted> {
        let plan = self.catalog.plans.declared(plan)?;
        let now = self.now();
        let subscription = match self.subscription(subscriber).await? {
            Some(mut subscription) => {
                subscription.start_trial(plan, now)?;
                subscription
            }
            None => Subscription::trial(subscriber, plan, now)?,
        };
        Ok(TrialStarted(subscription))
    }

    pub async fn subscribe(
        &self,
        subscriber: Subscriber,
        plan: Plan,
        seats: u32,
        return_path: &str,
    ) -> Result<Link> {
        let plan = self.catalog.plans.declared(plan)?;
        let subscription = self.subscription(subscriber).await?;
        let running = subscription
            .as_ref()
            .is_some_and(|held| held.standing(self.now()).renews_at_provider());
        if running {
            return Err(Error::Rule(PLAN_RUNNING.to_owned()));
        }
        self.checkout(
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
        let product = self.catalog.plans.declared_product(product)?;
        let subscription = self.subscription(subscriber).await?;
        self.checkout(
            subscriber,
            subscription,
            product.name(),
            quantity,
            false,
            return_path,
        )
        .await
    }

    /// For an owner that is gone, such as a deleted organization whose checkout completed
    /// after it; run from a job, which retries with the same key.
    pub async fn cancel_at_provider(
        &self,
        subscriber: Subscriber,
        key: &IdempotencyKey,
    ) -> Result<()> {
        let subscription = self.subscription(subscriber).await?;
        let Some(running) = subscription
            .as_ref()
            .and_then(Subscription::running_at_provider)
        else {
            return Ok(());
        };
        let cancellation = Cancellation {
            subscription: running,
            key,
        };
        self.catalog.setup.provider()?.cancel(&cancellation).await
    }

    pub async fn portal(&self, subscriber: Subscriber, return_path: &str) -> Result<Link> {
        let subscription = self.subscription(subscriber).await?;
        let provider_customer = subscription
            .as_ref()
            .and_then(Subscription::provider_customer)
            .ok_or_else(|| Error::Rule("Nothing has been paid for yet.".to_owned()))?;
        self.catalog
            .setup
            .portal(provider_customer, return_path)
            .await
    }

    async fn checkout(
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
        self.catalog
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

    pub(crate) async fn subscription(
        &self,
        subscriber: Subscriber,
    ) -> Result<Option<Subscription>> {
        Subscription::of(&self.db.read(), subscriber.uuid()).await
    }

    pub(crate) fn now(&self) -> Timestamp {
        self.db.clock().now().timestamp()
    }
}

const PLAN_RUNNING: &str = "A plan already renews; change it from the billing portal.";

impl<C: Context> Resolve<C> for Billing {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        let db = context.db();
        let billing = sofie_db::provided::<Catalog>(db)
            .map(|catalog| Self {
                catalog,
                db: db.clone(),
            })
            .ok_or_else(not_installed);
        std::future::ready(billing)
    }
}

/// The principal of the request's area is the owner.
impl<A: Area, M: Send> Resolve<RequestContext<A, M>> for Entitlements {
    async fn resolve(context: &mut RequestContext<A, M>) -> Result<Self> {
        let principal = Principal::<A>::resolve(context).await?;
        let billing = Billing::resolve(context).await?;
        billing
            .entitlements(Subscriber::new(principal.id().uuid()))
            .await
    }
}

fn not_installed() -> Error {
    Error::internal_message(
        "billing is not installed: `sofie gen` installs it once a module declares \
         `src/plans.rs`; a test calls `sofie_billing::install`",
    )
}

impl Entitled for Billing {
    fn held<'a>(
        &'a self,
        read: &'a Read,
        _area: &'static str,
        principal: Uuid,
    ) -> BoxFuture<'a, Result<Arc<[&'static str]>>> {
        Box::pin(async move {
            let subscription = Subscription::of(read, principal).await?;
            let plan = crate::entitlements::plan_held(
                self.catalog.plans,
                subscription.as_ref(),
                self.now(),
            );
            Ok(Arc::from(plan.held()))
        })
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

#[must_use = "the trial starts only when the handler returns it"]
pub struct TrialStarted(Subscription);

impl TrialStarted {
    #[must_use]
    pub fn ends_at(&self) -> Option<Timestamp> {
        self.0.trial_ends_at()
    }
}

#[must_use = "the seats are asked for only when the handler returns it"]
pub struct SeatsChangeAsked(Subscription);

impl Outcome for SeatsChangeAsked {
    fn plan(&self, plan: &mut CommitPlan<'_>) -> Result<()> {
        self.0.plan(plan)
    }

    fn committed(&mut self, stamp: Stamp) {
        self.0.committed(stamp);
    }
}

impl Outcome for TrialStarted {
    fn plan(&self, plan: &mut CommitPlan<'_>) -> Result<()> {
        self.0.plan(plan)
    }

    fn committed(&mut self, stamp: Stamp) {
        self.0.committed(stamp);
    }

    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(serde_json::json!({
            "trial_ends_at": self.0.trial_ends_at().map(|end| end.to_string()),
        })))
    }
}
