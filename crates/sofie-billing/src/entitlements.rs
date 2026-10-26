use sofie_core::{Error, Result, Timestamp};

use crate::billing::Billing;
use crate::plan::is_unlimited;
use crate::status::Standing;
use crate::subscription::Subscription;
use crate::usage::{self, Consumed};
use crate::{Limit, Plan, Plans, Quota, Seats, Subscriber};

/// What an owner's plan holds now; a handler takes it for the request's principal.
pub struct Entitlements {
    billing: Billing,
    subscriber: Subscriber,
    plan: &'static Plan,
    standing: Option<Standing>,
    seats: Seats,
    /// Where quota periods are counted from: the subscription's, else calendar periods in UTC.
    anchor: Timestamp,
}

impl Entitlements {
    pub(crate) fn of(
        billing: Billing,
        subscriber: Subscriber,
        subscription: Option<&Subscription>,
    ) -> Self {
        let now = billing.now();
        let plan = plan_held(billing.catalog.plans, subscription, now);
        let paying = subscription.filter(|subscription| subscription.plan_held(now).is_some());
        Self {
            subscriber,
            plan,
            standing: subscription.map(|subscription| subscription.standing(now)),
            seats: plan
                .seat_rule()
                .seats(paying.map_or(0, Subscription::seats)),
            anchor: paying.map_or(Timestamp::UNIX_EPOCH, Subscription::started_at),
            billing,
        }
    }

    #[must_use]
    pub fn plan(&self) -> &'static Plan {
        self.plan
    }

    /// `None` for an owner that never subscribed nor started a trial.
    #[must_use]
    pub fn standing(&self) -> Option<Standing> {
        self.standing
    }

    #[must_use]
    pub fn seats(&self) -> Seats {
        self.seats
    }

    /// Refuses one more when `count` already reach the plan's limit.
    pub fn allow_another(&self, limit: Limit, count: u64) -> Result<()> {
        let allowed = self.plan.limit(limit);
        if is_unlimited(allowed) || count < allowed {
            return Ok(());
        }
        Err(Error::Rule(format!(
            "Your plan allows at most {allowed} of `{}`.",
            limit.name()
        )))
    }

    /// In the period holding now.
    pub async fn used(&self, quota: Quota) -> Result<u64> {
        let start = self.period_start(quota)?;
        usage::used(&self.billing.db.read(), self.subscriber, quota, start).await
    }

    /// Counted once the handler's outcome commits, so a refused action uses nothing.
    pub async fn consume(&self, quota: Quota, amount: u64) -> Result<Consumed> {
        let allowed = self.plan.quota(quota);
        let start = self.period_start(quota)?;
        if !is_unlimited(allowed) {
            let used = usage::used(&self.billing.db.read(), self.subscriber, quota, start).await?;
            if used.saturating_add(amount) > allowed {
                return Err(Error::Rule(format!(
                    "Your plan's `{}` for this period is used up.",
                    quota.name()
                )));
            }
        }
        Ok(Consumed::new(
            self.subscriber.uuid(),
            quota.name(),
            start,
            amount,
            allowed,
        ))
    }

    fn period_start(&self, quota: Quota) -> Result<Timestamp> {
        usage::period_start(self.anchor, quota.period(), self.billing.now())
    }
}

/// A plan the code no longer declares falls back to the default, which is logged.
pub(crate) fn plan_held(
    plans: &'static Plans,
    subscription: Option<&Subscription>,
    now: Timestamp,
) -> &'static Plan {
    let Some(name) = subscription.and_then(|subscription| subscription.plan_held(now)) else {
        return plans.default_plan();
    };
    plans.plan(name).unwrap_or_else(|| {
        tracing::warn!(
            plan = name,
            "a subscription holds a plan the app no longer declares"
        );
        plans.default_plan()
    })
}
