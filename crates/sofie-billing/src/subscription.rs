use sofie_core::{
    Cast, ColumnReader, ColumnSpec, Error, Result, SqlType, SqlValue, Timestamp, Uuid,
};
use sofie_db::{Aggregate, Column, Field, Meta, Read, Select};

use crate::events::{SeatsAsked, SubscriptionChanged};
use crate::provider::SubscriptionChange;
use crate::status::Standing;
use crate::{Plan, Status, Subscriber};

const DAY_SECONDS: i64 = 24 * 60 * 60;
const TEXT: &[ColumnSpec] = &[ColumnSpec::new("", SqlType::Text)];

pub(crate) struct Subscription {
    meta: Meta,
    subscriber: Uuid,
    plan: String,
    status: Status,
    seats: i64,
    /// Kept once set, so an owner gets one trial.
    trial_ends_at: Option<Timestamp>,
    /// Where quota periods are counted from: when this trial or provider subscription began,
    /// kept across renewals, which move `period_start`. Unset on a row written before it
    /// existed, which counts from the `period_start` it held then.
    started_at: Option<Timestamp>,
    period_start: Timestamp,
    period_ends_at: Option<Timestamp>,
    cancels_at_period_end: bool,
    provider: Option<String>,
    provider_customer: Option<String>,
    reference: Option<String>,
    /// The provider's time of the last change followed, so an older one arriving late is dropped.
    provider_changed_at: Option<Timestamp>,
    created_at: Timestamp,
    changed_at: Timestamp,
    /// The latest count asked of the provider's subscription held, so an older ask still
    /// queued, or one of a subscription replaced since, is not sent.
    seats_asked: Option<i64>,
}

const SUBSCRIBER: Column<Subscription, Uuid> = Column::new("subscriber");

pub(crate) enum Followed {
    Changed,
    Ignored(&'static str),
}

impl Subscription {
    pub(crate) async fn of(read: &Read, subscriber: Uuid) -> Result<Option<Self>> {
        Self::query()
            .where_(SUBSCRIBER.eq(subscriber))
            .first(read)
            .await
    }

    fn new(subscriber: Uuid, plan: &str, status: Status, now: Timestamp) -> Result<Self> {
        Ok(Self {
            meta: Meta::new()?,
            subscriber,
            plan: plan.to_owned(),
            status,
            seats: 1,
            trial_ends_at: None,
            started_at: Some(now),
            period_start: now,
            period_ends_at: None,
            cancels_at_period_end: false,
            provider: None,
            provider_customer: None,
            reference: None,
            provider_changed_at: None,
            created_at: now,
            changed_at: now,
            seats_asked: None,
        })
    }

    pub(crate) fn trial(subscriber: Subscriber, plan: &Plan, now: Timestamp) -> Result<Self> {
        let mut subscription = Self::new(subscriber.uuid(), plan.name(), Status::Trialing, now)?;
        subscription.trial_ends_at = Some(trial_end(plan, now)?);
        subscription.raise()?;
        Ok(subscription)
    }

    pub(crate) fn start_trial(&mut self, plan: &Plan, now: Timestamp) -> Result<()> {
        if self.plan_held(now).is_some() {
            return Err(Error::Rule("A subscription is already running.".to_owned()));
        }
        if self.trial_ends_at.is_some() {
            return Err(Error::Rule("The trial was already used.".to_owned()));
        }
        plan.name().clone_into(&mut self.plan);
        self.status = Status::Trialing;
        self.seats = 1;
        self.trial_ends_at = Some(trial_end(plan, now)?);
        self.started_at = Some(now);
        self.period_start = now;
        self.period_ends_at = None;
        self.cancels_at_period_end = false;
        self.provider = None;
        self.reference = None;
        self.seats_asked = None;
        self.changed_at = now;
        self.raise()
    }

    pub(crate) fn from_provider(
        provider: &'static str,
        change: &SubscriptionChange,
        status: Status,
        plan: &str,
        now: Timestamp,
    ) -> Result<Self> {
        let mut subscription = Self::new(change.subscriber.uuid(), plan, status, now)?;
        subscription.take(provider, change, status, plan, now)?;
        Ok(subscription)
    }

    /// The provider is the truth on payment, so its state is taken unless the machine forbids the move.
    pub(crate) fn follow(
        &mut self,
        provider: &'static str,
        change: &SubscriptionChange,
        status: Status,
        plan: &str,
        now: Timestamp,
    ) -> Result<Followed> {
        if self
            .provider_changed_at
            .is_some_and(|held| change.occurred_at < held)
        {
            return Ok(Followed::Ignored("an older change than the one held"));
        }
        let same = self.is_held(change);
        if !same && status == Status::Canceled {
            return Ok(Followed::Ignored(
                "another subscription than the one held, canceled",
            ));
        }
        if !same && self.reference.is_some() && self.status != Status::Canceled {
            tracing::warn!(
                subscriber = %self.subscriber,
                "the provider started a second running subscription; following the newer one"
            );
        }
        // Providers keep no delivery order within a second, and a cancellation and a change
        // before it often share one.
        let resurrected = self.status == Status::Canceled
            && status != Status::Canceled
            && self.provider_changed_at == Some(change.occurred_at);
        if same && resurrected {
            return Ok(Followed::Ignored(
                "a change in the same second as the cancellation",
            ));
        }
        if same && status != self.status && !self.status.may_become(status) {
            return Ok(Followed::Ignored(
                "a move the subscription's states do not allow",
            ));
        }
        self.take(provider, change, status, plan, now)?;
        Ok(Followed::Changed)
    }

    pub(crate) fn is_held(&self, change: &SubscriptionChange) -> bool {
        self.reference.as_deref() == Some(change.subscription.as_str())
    }

    fn take(
        &mut self,
        provider: &'static str,
        change: &SubscriptionChange,
        status: Status,
        plan: &str,
        now: Timestamp,
    ) -> Result<()> {
        self.started_at = Some(if self.is_held(change) {
            self.started_at()
        } else {
            change.period_start
        });
        plan.clone_into(&mut self.plan);
        self.status = status;
        self.seats = i64::from(change.quantity);
        if change.trial_ends_at.is_some() {
            self.trial_ends_at = change.trial_ends_at;
        }
        self.period_start = change.period_start;
        self.period_ends_at = change.period_ends_at;
        self.cancels_at_period_end = change.cancels_at_period_end;
        self.provider = Some(provider.to_owned());
        self.provider_customer = Some(change.provider_customer.clone());
        if !self.is_held(change) {
            self.seats_asked = None;
        }
        self.reference = Some(change.subscription.clone());
        self.provider_changed_at = Some(change.occurred_at);
        self.changed_at = now;
        self.raise()
    }

    pub(crate) fn ask_seats(&mut self, seats: u32, now: Timestamp) -> Result<()> {
        if self.reference.is_none() {
            return Err(Error::Rule("Nothing has been paid for yet.".to_owned()));
        }
        self.seats_asked = Some(i64::from(seats));
        self.changed_at = now;
        self.meta.raise(SeatsAsked {
            subscriber: self.subscriber,
            seats,
        })
    }

    /// The provider's subscription while `seats` is still the latest count asked of it.
    pub(crate) fn asked_of_provider(&self, seats: u32) -> Option<&str> {
        let latest = self.seats_asked == Some(i64::from(seats));
        self.reference.as_deref().filter(|_| latest)
    }

    /// The provider's subscription while it still grants the plan or bills.
    pub(crate) fn running_at_provider(&self) -> Option<&str> {
        self.reference
            .as_deref()
            .filter(|_| self.status != Status::Canceled)
    }

    pub(crate) fn seats_asked(&self) -> Option<u32> {
        self.seats_asked.and_then(|seats| u32::try_from(seats).ok())
    }

    /// The plan held now; a trial without a card ends by itself, as nobody bills it.
    pub(crate) fn plan_held(&self, now: Timestamp) -> Option<&str> {
        if !self.status.grants_plan() {
            return None;
        }
        let trial_over = self.status == Status::Trialing
            && self.provider.is_none()
            && self.trial_ends_at.is_none_or(|end| end <= now);
        (!trial_over).then_some(self.plan.as_str())
    }

    /// A trial that ended by itself reads as canceled, though nothing wrote that.
    pub(crate) fn standing(&self, now: Timestamp) -> Standing {
        let held = self.plan_held(now).is_some();
        Standing {
            status: if held { self.status } else { Status::Canceled },
            trial_ends_at: self.trial_ends_at,
            renews_at: self.period_ends_at.filter(|_| held),
            cancels_at_period_end: held && self.cancels_at_period_end,
            renews_at_provider: held && self.reference.is_some() && !self.cancels_at_period_end,
        }
    }

    pub(crate) fn plan_name(&self) -> &str {
        &self.plan
    }

    pub(crate) fn seats(&self) -> u32 {
        u32::try_from(self.seats).unwrap_or(u32::MAX)
    }

    pub(crate) fn started_at(&self) -> Timestamp {
        self.started_at.unwrap_or(self.period_start)
    }

    pub(crate) fn trial_ends_at(&self) -> Option<Timestamp> {
        self.trial_ends_at
    }

    pub(crate) fn provider_customer(&self) -> Option<&str> {
        self.provider_customer.as_deref()
    }

    fn raise(&mut self) -> Result<()> {
        let changed = SubscriptionChanged {
            subscriber: self.subscriber,
            plan: self.plan.clone(),
            status: self.status,
        };
        self.meta.raise(changed)
    }
}

fn trial_end(plan: &Plan, now: Timestamp) -> Result<Timestamp> {
    if plan.trial() == 0 {
        return Err(Error::Rule(format!("Plan `{}` has no trial.", plan.name())));
    }
    let length = jiff::SignedDuration::from_secs(i64::from(plan.trial()) * DAY_SECONDS);
    now.checked_add(length)
        .map_err(|error| Error::internal("computing a trial's end", error))
}

impl Aggregate for Subscription {
    const NAME: &'static str = "subscription";
    const MODULE: &'static str = "sofie_billing";
    const FIELDS: &'static [Field] = &[
        Field::new("subscriber", Uuid::COLUMNS, false),
        Field::new("plan", String::COLUMNS, false),
        Field::new("status", TEXT, false),
        Field::new("seats", i64::COLUMNS, false),
        Field::new("trial_ends_at", Timestamp::COLUMNS, true),
        Field::new("period_start", Timestamp::COLUMNS, false),
        Field::new("period_ends_at", Timestamp::COLUMNS, true),
        Field::new("cancels_at_period_end", bool::COLUMNS, false),
        Field::new("provider", String::COLUMNS, true),
        Field::new("provider_customer", String::COLUMNS, true),
        Field::new("reference", String::COLUMNS, true),
        Field::new("provider_changed_at", Timestamp::COLUMNS, true),
        Field::new("created_at", Timestamp::COLUMNS, false),
        Field::new("changed_at", Timestamp::COLUMNS, false),
        Field::new("seats_asked", i64::COLUMNS, true),
        Field::new("started_at", Timestamp::COLUMNS, true),
    ];
    const UNIQUE: &'static [&'static str] = &["subscriber"];
    /// So an owned area's principal starts a trial for itself and nobody else.
    const OWNER: Option<&'static str> = Some("subscriber");

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn owner(&self) -> Option<Uuid> {
        Some(self.subscriber)
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.subscriber.encode(out);
        self.plan.encode(out);
        out.push(SqlValue::Text(self.status.as_str().to_owned()));
        self.seats.encode(out);
        self.trial_ends_at.encode(out);
        self.period_start.encode(out);
        self.period_ends_at.encode(out);
        self.cancels_at_period_end.encode(out);
        self.provider.encode(out);
        self.provider_customer.encode(out);
        self.reference.encode(out);
        self.provider_changed_at.encode(out);
        self.created_at.encode(out);
        self.changed_at.encode(out);
        self.seats_asked.encode(out);
        self.started_at.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            subscriber: Uuid::decode(row)?,
            plan: String::decode(row)?,
            status: Status::parse(row.text()?)?,
            seats: i64::decode(row)?,
            trial_ends_at: Option::decode(row)?,
            period_start: Timestamp::decode(row)?,
            period_ends_at: Option::decode(row)?,
            cancels_at_period_end: bool::decode(row)?,
            provider: Option::decode(row)?,
            provider_customer: Option::decode(row)?,
            reference: Option::decode(row)?,
            provider_changed_at: Option::decode(row)?,
            created_at: Timestamp::decode(row)?,
            changed_at: Timestamp::decode(row)?,
            seats_asked: Option::decode(row)?,
            started_at: Option::decode(row)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRO: Plan = Plan::new("pro").trial_days(14).per_seat();

    fn change(status: Status, now: Timestamp) -> SubscriptionChange {
        SubscriptionChange {
            subscriber: Subscriber::new(Uuid::from_u128(1)),
            subscription: "sub_1".to_owned(),
            provider_customer: "cus_1".to_owned(),
            price: "price_pro".to_owned(),
            status: Some(status),
            quantity: 4,
            period_start: now,
            period_ends_at: None,
            trial_ends_at: None,
            cancels_at_period_end: true,
            occurred_at: now,
        }
    }

    #[test]
    fn a_trial_after_a_canceled_paid_subscription_keeps_none_of_its_seats_or_cancel() {
        let now: Timestamp = "2026-10-14T10:00:00Z".parse().unwrap();
        let change = change(Status::Canceled, now);
        let mut subscription =
            Subscription::from_provider("stripe", &change, Status::Canceled, "pro", now).unwrap();

        subscription.start_trial(&PRO, now).unwrap();

        assert_eq!(subscription.plan_held(now), Some("pro"));
        assert_eq!(subscription.seats(), 1);
        assert!(!subscription.standing(now).cancels_at_period_end());
    }

    #[test]
    fn a_change_in_the_second_of_the_cancellation_does_not_bring_it_back() {
        let now: Timestamp = "2026-10-14T10:00:00Z".parse().unwrap();
        let canceled = change(Status::Canceled, now);
        let mut subscription =
            Subscription::from_provider("stripe", &canceled, Status::Canceled, "pro", now).unwrap();

        let same_second = subscription
            .follow(
                "stripe",
                &change(Status::Active, now),
                Status::Active,
                "pro",
                now,
            )
            .unwrap();

        assert!(matches!(same_second, Followed::Ignored(_)));
        assert_eq!(subscription.plan_held(now), None);
    }

    #[test]
    fn a_renewal_keeps_where_quotas_are_counted_from() {
        let started: Timestamp = "2026-10-14T10:00:00Z".parse().unwrap();
        let renewed: Timestamp = "2026-11-14T10:00:00Z".parse().unwrap();
        let mut subscription = Subscription::from_provider(
            "stripe",
            &change(Status::Active, started),
            Status::Active,
            "pro",
            started,
        )
        .unwrap();

        subscription
            .follow(
                "stripe",
                &change(Status::Active, renewed),
                Status::Active,
                "pro",
                renewed,
            )
            .unwrap();

        assert_eq!(subscription.started_at(), started);
    }
}
