use std::time::Duration;

use sofie_core::{Error, Result};

/// Each counted per caller key in a sliding window of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Rule {
    SignIn,
    /// Locks signing in to the account for the failing caller.
    Account,
    /// Locks signing in to the account for everyone, so a looser count.
    AccountTotal,
    SignUp,
    Scan,
    /// As guessing other owners' ids is.
    Probe,
    Refused,
    Malformed,
    Throttled,
    Protocol,
}

/// Signals before the caller is slowed down; twice as many block, half as many are reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limit {
    pub(crate) count: u32,
    pub(crate) window: Duration,
}

impl Rule {
    pub(crate) const ALL: [Self; 10] = [
        Self::SignIn,
        Self::Account,
        Self::AccountTotal,
        Self::SignUp,
        Self::Scan,
        Self::Probe,
        Self::Refused,
        Self::Malformed,
        Self::Throttled,
        Self::Protocol,
    ];

    #[must_use]
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::SignIn => "sign_in",
            Self::Account => "account",
            Self::AccountTotal => "account_total",
            Self::SignUp => "sign_up",
            Self::Scan => "scan",
            Self::Probe => "probe",
            Self::Refused => "refused",
            Self::Malformed => "malformed",
            Self::Throttled => "throttled",
            Self::Protocol => "protocol",
        }
    }

    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::SignIn => "failed sign-ins from one caller",
            Self::Account => "failed sign-ins to one account from one caller",
            Self::AccountTotal => "failed sign-ins to one account from every caller",
            Self::SignUp => "a burst of sign-ups from one caller",
            Self::Scan => "requests for paths no route serves",
            Self::Probe => "records not found on matched routes",
            Self::Refused => "cross-site, unsigned or unpermitted requests",
            Self::Malformed => "malformed or oversized bodies",
            Self::Throttled => "requests repeated over a rate limit",
            Self::Protocol => "connections failed on protocol errors",
        }
    }

    pub(crate) fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|rule| rule.name() == name)
    }

    /// Generous enough that mistyping, a stale bookmark or a flaky connection stays far below.
    fn default_limit(self) -> Limit {
        let (count, seconds) = match self {
            Self::SignIn => (20, 600),
            Self::Account => (10, 900),
            Self::AccountTotal => (100, 900),
            Self::SignUp => (10, 3600),
            Self::Scan | Self::Throttled => (30, 60),
            Self::Probe | Self::Malformed => (20, 60),
            Self::Refused => (10, 60),
            Self::Protocol => (5, 60),
        };
        Limit {
            count,
            window: Duration::from_secs(seconds),
        }
    }
}

/// `SOFIE_SHIELD_THRESHOLDS`: `scan=60, member.probe=40`; an area's own count wins.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Thresholds {
    counts: Vec<(Option<String>, Rule, u32)>,
}

impl Thresholds {
    pub fn parse(text: &str) -> Result<Self> {
        let counts = text
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(threshold)
            .collect::<Result<_>>()?;
        Ok(Self { counts })
    }

    pub(crate) fn limit(&self, area: Option<&str>, rule: Rule) -> Limit {
        let set = |wanted: Option<&str>| {
            self.counts
                .iter()
                .find(|(area, named, _)| *named == rule && area.as_deref() == wanted)
                .map(|(_, _, count)| *count)
        };
        let default = rule.default_limit();
        let count = area
            .and_then(|area| set(Some(area)))
            .or_else(|| set(None))
            .unwrap_or(default.count);
        Limit { count, ..default }
    }

    pub(crate) fn check_areas(&self, areas: &[&str]) -> Result<()> {
        match self
            .counts
            .iter()
            .filter_map(|(area, ..)| area.as_deref())
            .find(|area| !areas.contains(area))
        {
            Some(unknown) => Err(Error::internal_message(format!(
                "shield threshold for area `{unknown}`, which the app does not have"
            ))),
            None => Ok(()),
        }
    }
}

fn threshold(entry: &str) -> Result<(Option<String>, Rule, u32)> {
    let refused = || {
        Error::internal_message(format!(
            "shield threshold `{entry}` is not `rule=count` or `area.rule=count` with a count above 0"
        ))
    };
    let (name, count) = entry.split_once('=').ok_or_else(refused)?;
    let count: u32 = count.trim().parse().map_err(|_| refused())?;
    let (area, rule) = match name.trim().rsplit_once('.') {
        Some((area, rule)) => (Some(area.to_owned()), rule),
        None => (None, name.trim()),
    };
    let rule = Rule::named(rule).ok_or_else(refused)?;
    if count == 0 {
        return Err(refused());
    }
    Ok((area, rule, count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_area_count_beats_the_general_one_which_beats_the_default() {
        let thresholds = Thresholds::parse("scan=60, member.scan=90, probe = 5").unwrap();

        assert_eq!(thresholds.limit(Some("member"), Rule::Scan).count, 90);
        assert_eq!(thresholds.limit(Some("web"), Rule::Scan).count, 60);
        assert_eq!(thresholds.limit(None, Rule::Probe).count, 5);
        assert_eq!(
            thresholds.limit(None, Rule::SignIn),
            Rule::SignIn.default_limit()
        );
    }

    #[test]
    fn a_malformed_entry_or_an_unknown_area_is_refused() {
        for entry in ["scan", "scan=0", "scan=-1", "scanning=3", "web.=3"] {
            let error = Thresholds::parse(entry).unwrap_err();
            assert!(error.to_string().contains(entry), "{error}");
        }
        let thresholds = Thresholds::parse("console.probe=3").unwrap();

        assert!(thresholds.check_areas(&["console"]).is_ok());
        assert!(thresholds.check_areas(&["web"]).is_err());
    }
}
