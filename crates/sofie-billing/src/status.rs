use serde::{Deserialize, Serialize};
use sofie_core::{Error, Result, Timestamp};

/// A subscription's state; the provider's own states are mapped onto these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Trialing,
    Active,
    /// A payment failed; the plan is kept while the provider retries.
    PastDue,
    Canceled,
}

impl Status {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trialing => "trialing",
            Self::Active => "active",
            Self::PastDue => "past_due",
            Self::Canceled => "canceled",
        }
    }

    pub(crate) fn parse(text: &str) -> Result<Self> {
        Ok(match text {
            "trialing" => Self::Trialing,
            "active" => Self::Active,
            "past_due" => Self::PastDue,
            "canceled" => Self::Canceled,
            other => {
                return Err(Error::internal_message(format!(
                    "a stored subscription has the unknown status `{other}`"
                )));
            }
        })
    }

    /// A canceled subscription comes back only by paying, never by a second trial.
    #[must_use]
    pub const fn may_become(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Trialing,
                Self::Active | Self::PastDue | Self::Canceled
            ) | (Self::Active, Self::PastDue | Self::Canceled)
                | (Self::PastDue, Self::Active | Self::Canceled)
                | (Self::Canceled, Self::Active)
        )
    }

    pub(crate) const fn grants_plan(self) -> bool {
        !matches!(self, Self::Canceled)
    }
}

/// Where an owner that ever subscribed or started a trial stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Standing {
    pub(crate) status: Status,
    pub(crate) trial_ends_at: Option<Timestamp>,
    pub(crate) renews_at: Option<Timestamp>,
    pub(crate) cancels_at_period_end: bool,
    pub(crate) renews_at_provider: bool,
}

impl Standing {
    #[must_use]
    pub fn status(self) -> Status {
        self.status
    }

    #[must_use]
    pub fn trial_ends_at(self) -> Option<Timestamp> {
        self.trial_ends_at
    }

    #[must_use]
    pub fn renews_at(self) -> Option<Timestamp> {
        self.renews_at
    }

    #[must_use]
    pub fn cancels_at_period_end(self) -> bool {
        self.cancels_at_period_end
    }

    /// The provider charges again unless it is canceled there, as before its owner is deleted.
    #[must_use]
    pub fn renews_at_provider(self) -> bool {
        self.renews_at_provider
    }
}

#[cfg(test)]
mod tests {
    use super::Status::{Active, Canceled, PastDue, Trialing};

    #[test]
    fn a_subscription_moves_only_along_the_machine() {
        assert!(Trialing.may_become(Active));
        assert!(Active.may_become(PastDue));
        assert!(PastDue.may_become(Active));
        assert!(Canceled.may_become(Active));
        assert!(!Canceled.may_become(Trialing));
        assert!(!Active.may_become(Trialing));
        assert!(!Active.may_become(Active));
    }
}
