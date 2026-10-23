use sofie_core::{Error, Result};

use crate::entitlement::{Entitlement, Limit, Quota, UNLIMITED, is_entitlement_name};

/// Each name well formed and given once; a macro, since a const fn takes no closure.
macro_rules! assert_names {
    (
        $items:expr,
        |$item:ident| $name:expr,
        $well_formed:ident,
        $malformed:expr,
        $repeated:literal
    ) => {{
        let items = $items;
        let mut index = 0;
        while index < items.len() {
            let $item = &items[index];
            let name = $name;
            assert!($well_formed(name), "{}", $malformed);
            let mut earlier = 0;
            while earlier < index {
                let $item = &items[earlier];
                assert!(!same($name, name), $repeated);
                earlier += 1;
            }
            index += 1;
        }
    }};
}

/// What a plan grants, declared as a constant in a module's `src/plans.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    name: &'static str,
    grants: &'static [Entitlement],
    limits: &'static [(Limit, u64)],
    quotas: &'static [(Quota, u64)],
    trial_days: u32,
    seats: SeatRule,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeatRule {
    Unlimited,
    Fixed(u32),
    /// As many as the subscription's quantity, which the payment provider bills.
    Purchased,
}

impl SeatRule {
    pub(crate) const fn seats(self, purchased: u32) -> Seats {
        match self {
            Self::Unlimited => Seats::Unlimited,
            Self::Fixed(seats) => Seats::AtMost(seats),
            Self::Purchased => Seats::AtMost(purchased),
        }
    }
}

/// The seats an owner may fill now; what counts as one is the app's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seats {
    Unlimited,
    AtMost(u32),
}

impl Seats {
    #[must_use]
    pub fn allows(self, filled: u32) -> bool {
        match self {
            Self::Unlimited => true,
            Self::AtMost(seats) => filled <= seats,
        }
    }
}

/// Bought once rather than subscribed to, such as a pack of credits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Product(&'static str);

impl Product {
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        self.0
    }
}

impl Plan {
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            grants: &[],
            limits: &[],
            quotas: &[],
            trial_days: 0,
            seats: SeatRule::Unlimited,
        }
    }

    #[must_use]
    pub const fn grants(self, grants: &'static [Entitlement]) -> Self {
        Self { grants, ..self }
    }

    /// `UNLIMITED` for no end; a limit the plan leaves out allows none.
    #[must_use]
    pub const fn limits(self, limits: &'static [(Limit, u64)]) -> Self {
        Self { limits, ..self }
    }

    #[must_use]
    pub const fn quotas(self, quotas: &'static [(Quota, u64)]) -> Self {
        Self { quotas, ..self }
    }

    #[must_use]
    pub const fn trial_days(self, trial_days: u32) -> Self {
        Self { trial_days, ..self }
    }

    #[must_use]
    pub const fn seats(self, seats: u32) -> Self {
        Self {
            seats: SeatRule::Fixed(seats),
            ..self
        }
    }

    #[must_use]
    pub const fn per_seat(self) -> Self {
        Self {
            seats: SeatRule::Purchased,
            ..self
        }
    }

    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    #[must_use]
    pub const fn trial(&self) -> u32 {
        self.trial_days
    }

    #[must_use]
    pub const fn seat_rule(&self) -> SeatRule {
        self.seats
    }

    #[must_use]
    pub fn allows(&self, entitlement: Entitlement) -> bool {
        self.grants.contains(&entitlement)
    }

    #[must_use]
    pub fn limit(&self, limit: Limit) -> u64 {
        allowance(self.limits, limit)
    }

    #[must_use]
    pub fn quota(&self, quota: Quota) -> u64 {
        allowance(self.quotas, quota)
    }

    pub(crate) fn held(&self) -> Vec<&'static str> {
        let grants = self.grants.iter().map(|entitlement| entitlement.name());
        let limits = self
            .limits
            .iter()
            .filter(|(_, amount)| *amount > 0)
            .map(|(limit, _)| limit.name());
        let quotas = self
            .quotas
            .iter()
            .filter(|(_, amount)| *amount > 0)
            .map(|(quota, _)| quota.name());
        grants.chain(limits).chain(quotas).collect()
    }
}

fn allowance<T: PartialEq + Copy>(allowances: &[(T, u64)], wanted: T) -> u64 {
    allowances
        .iter()
        .find(|(declared, _)| *declared == wanted)
        .map_or(0, |(_, amount)| *amount)
}

/// Checked when the constant is built, so a malformed catalog fails the build.
#[derive(Clone, Copy, Debug)]
pub struct Plans {
    default: Plan,
    others: &'static [Plan],
    products: &'static [Product],
}

impl Plans {
    /// `default` is held by every owner without a subscription that grants another.
    #[must_use]
    pub const fn new(default: Plan, others: &'static [Plan]) -> Self {
        check(&default);
        assert_names!(
            others,
            |plan| plan.name,
            is_plan_name,
            PLAN_NAME,
            "two plans share a name"
        );
        let mut index = 0;
        while index < others.len() {
            check(&others[index]);
            assert!(
                !same(others[index].name, default.name),
                "two plans share a name"
            );
            index += 1;
        }
        Self {
            default,
            others,
            products: &[],
        }
    }

    #[must_use]
    pub const fn products(self, products: &'static [Product]) -> Self {
        assert_names!(
            products,
            |product| product.0,
            is_plan_name,
            "a product is named in snake case, such as `credits_100`",
            "two products share a name"
        );
        Self { products, ..self }
    }

    #[must_use]
    pub const fn default_plan(&self) -> &Plan {
        &self.default
    }

    pub fn all(&self) -> impl Iterator<Item = &Plan> {
        std::iter::once(&self.default).chain(self.others)
    }

    #[must_use]
    pub fn plan(&self, name: &str) -> Option<&Plan> {
        self.all().find(|plan| plan.name == name)
    }

    #[must_use]
    pub fn product(&self, name: &str) -> Option<Product> {
        self.products
            .iter()
            .copied()
            .find(|product| product.0 == name)
    }

    pub(crate) fn declared(&self, plan: Plan) -> Result<&Plan> {
        self.plan(plan.name)
            .ok_or_else(|| undeclared("plan", plan.name))
    }

    pub(crate) fn declared_product(&self, product: Product) -> Result<Product> {
        self.product(product.0)
            .ok_or_else(|| undeclared("product", product.0))
    }
}

fn undeclared(kind: &str, name: &str) -> Error {
    Error::internal_message(format!("{kind} `{name}` is not in the app's plans"))
}

const PLAN_NAME: &str = "a plan is named in snake case, such as `pro` or `team_yearly`";

const fn check(plan: &Plan) {
    assert!(is_plan_name(plan.name), "{}", PLAN_NAME);
    assert_names!(
        plan.grants,
        |grant| grant.name(),
        is_entitlement_name,
        "an entitlement is named `<module>.<name>` in snake case",
        "a plan grants an entitlement twice"
    );
    assert_names!(
        plan.limits,
        |limit| limit.0.name(),
        is_entitlement_name,
        "a limit is named `<module>.<name>` in snake case",
        "a plan sets a limit twice"
    );
    assert_names!(
        plan.quotas,
        |quota| quota.0.name(),
        is_entitlement_name,
        "a quota is named `<module>.<name>` in snake case",
        "a plan sets a quota twice"
    );
}

const fn is_plan_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_lowercase() {
        return false;
    }
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_') {
            return false;
        }
        index += 1;
    }
    true
}

const fn same(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

pub(crate) const fn is_unlimited(amount: u64) -> bool {
    amount == UNLIMITED
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Period;

    const EXPORT: Entitlement = Entitlement::new("project.export");
    const PROJECTS: Limit = Limit::new("project.projects");
    const CALLS: Quota = Quota::new("project.api_calls", Period::Month);
    const FREE: Plan = Plan::new("free").limits(&[(PROJECTS, 3)]);
    const PRO: Plan = Plan::new("pro")
        .grants(&[EXPORT])
        .limits(&[(PROJECTS, UNLIMITED)])
        .quotas(&[(CALLS, 1000)])
        .trial_days(14);
    const PLANS: Plans = Plans::new(FREE, &[PRO]);

    #[test]
    fn a_plan_holds_its_switches_and_allowances_above_zero() {
        assert!(PRO.allows(EXPORT));
        assert!(!FREE.allows(EXPORT));
        assert_eq!(FREE.limit(PROJECTS), 3);
        assert_eq!(FREE.quota(CALLS), 0);
        assert_eq!(
            PRO.held(),
            ["project.export", "project.projects", "project.api_calls"]
        );
        assert_eq!(PLANS.plan("pro").map(Plan::name), Some("pro"));
        assert_eq!(PLANS.default_plan().name(), "free");
        assert!(is_unlimited(PRO.limit(PROJECTS)));
    }
}
