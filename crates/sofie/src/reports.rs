use std::fmt::Write as _;
use std::time::Duration;

use sofie_core::{Clock, Date, Error, Result, Timestamp};
use sofie_db::worker::Worker;
use sofie_db::{Cohort, Db, Period, Prune, Retention, cohorts};

const KEEP_DAYS: &str = "SOFIE_PRODUCT_EVENTS_KEEP_DAYS";
/// A year and a month, so a cohort can be compared with the same month a year before.
const DEFAULT_KEEP_DAYS: u64 = 400;
const SECONDS_PER_DAY: u64 = 86_400;
/// About 10,000 years: further back than a `Timestamp` reaches would fail every prune.
const MAX_KEEP_DAYS: u64 = 3_650_000;
const PRUNE_EVERY: Duration = Duration::from_hours(1);

/// Without `--since`, this many periods back from now.
const PERIODS_SHOWN: u64 = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Asked {
    start: String,
    returning: String,
    period: Period,
    since: Option<Date>,
}

impl Asked {
    pub(crate) fn parse(options: &[&str]) -> Result<Self> {
        let (mut start, mut returning, mut period, mut since) = (None, None, None, None);
        let mut rest = options;
        while let [flag, value, remaining @ ..] = rest {
            match *flag {
                "--start" => start = Some((*value).to_owned()),
                "--returning" => returning = Some((*value).to_owned()),
                "--period" => {
                    period = Some(Period::parse(value).ok_or_else(|| {
                        Error::internal_message(format!(
                            "`{value}` is not a period; use day, week or month"
                        ))
                    })?);
                }
                "--since" => {
                    since = Some(value.parse::<Date>().map_err(|error| {
                        Error::internal(format!("`{value}` is not a date like 2026-01-31"), error)
                    })?);
                }
                _ => break,
            }
            rest = remaining;
        }
        let (Some(start), Some(returning), []) = (start, returning, rest) else {
            return Err(Error::internal_message(
                "usage: report retention --start EVENT --returning EVENT \
                 [--period day|week|month] [--since DATE]",
            ));
        };
        Ok(Self {
            start,
            returning,
            period: period.unwrap_or(Period::Week),
            since,
        })
    }

    fn since(&self, now: Timestamp) -> Result<Timestamp> {
        if let Some(date) = self.since {
            return format!("{date}T00:00:00Z")
                .parse()
                .map_err(|error| Error::internal("reading --since", error));
        }
        let days = match self.period {
            Period::Day => 1,
            Period::Week => 7,
            Period::Month => 31,
        };
        now.checked_sub(Duration::from_hours(24 * days * PERIODS_SHOWN))
            .map_err(|error| Error::internal("counting back from now", error))
    }
}

/// How long product events are kept, from `SOFIE_PRODUCT_EVENTS_KEEP_DAYS`.
pub(crate) fn kept() -> Result<Duration> {
    keeping(sofie_core::environment_variable(KEEP_DAYS)?.as_deref())
}

fn keeping(days: Option<&str>) -> Result<Duration> {
    let Some(days) = days else {
        return Ok(Duration::from_secs(DEFAULT_KEEP_DAYS * SECONDS_PER_DAY));
    };
    days.trim()
        .parse::<u64>()
        .ok()
        .filter(|days| (1..=MAX_KEEP_DAYS).contains(days))
        .map(|days| Duration::from_secs(days * SECONDS_PER_DAY))
        .ok_or_else(|| {
            Error::internal_message(format!(
                "{KEEP_DAYS} is not a whole number of days from 1 to {MAX_KEEP_DAYS}"
            ))
        })
}

/// Every hour, events older than `kept` are deleted.
pub(crate) fn forgetting(worker: Worker, kept: Duration) -> Worker {
    worker.every(
        "sofie.product_events.prune",
        PRUNE_EVERY,
        move |prune: Prune, clock: Clock| async move {
            let before = clock.now().checked_sub(kept)?;
            prune.product_events(before).await.map(drop)
        },
    )
}

pub(crate) async fn print(db: &Db, asked: &Asked) -> Result<()> {
    let since = asked.since(Clock::system().now().timestamp())?;
    let found = cohorts(
        db,
        &Retention {
            start: &asked.start,
            returning: &asked.returning,
            period: asked.period,
            since,
        },
    )
    .await?;
    print!("{}", table(asked, since, &found));
    Ok(())
}

/// Each cell is the share of the cohort that returned in that period.
fn table(asked: &Asked, since: Timestamp, cohorts: &[Cohort]) -> String {
    let mut text = format!(
        "who raised `{}` again after `{}`, cohorts by {} since {} (UTC)\n",
        asked.returning,
        asked.start,
        asked.period.name(),
        since.strftime("%Y-%m-%d")
    );
    let columns = cohorts
        .iter()
        .map(|cohort| cohort.returned.len())
        .max()
        .unwrap_or_default();
    let _ = write!(text, "{:<10} {:>6}", "cohort", "size");
    for column in 0..columns {
        let _ = write!(text, " {column:>5}");
    }
    text.push('\n');
    for cohort in cohorts {
        let _ = write!(text, "{:<10} {:>6}", cohort.start, cohort.size);
        for returned in &cohort.returned {
            let _ = write!(text, " {:>4}%", returned * 100 / cohort.size.max(1));
        }
        text.push('\n');
    }
    if cohorts.is_empty() {
        text.push_str("no principal raised the start event in that time\n");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_events_and_defaults_to_weeks() {
        let asked = Asked::parse(&["--start", "signed_up", "--returning", "ordered"]).unwrap();
        let monthly = Asked::parse(&[
            "--returning",
            "ordered",
            "--start",
            "signed_up",
            "--period",
            "month",
            "--since",
            "2026-01-01",
        ])
        .unwrap();

        assert_eq!(asked.period, Period::Week);
        assert_eq!(monthly.period, Period::Month);
        assert_eq!(monthly.since, Some("2026-01-01".parse().unwrap()));
        assert!(Asked::parse(&["--start", "signed_up"]).is_err());
        assert!(Asked::parse(&["--start", "a", "--returning", "b", "--period", "year"]).is_err());
    }

    #[test]
    fn the_days_kept_default_and_never_overflow() {
        assert_eq!(keeping(None).unwrap(), Duration::from_hours(24 * 400));
        assert_eq!(
            keeping(Some(" 30 ")).unwrap(),
            Duration::from_hours(24 * 30)
        );
        for refused in ["0", "-1", "many", &(MAX_KEEP_DAYS + 1).to_string()] {
            let error = keeping(Some(refused)).unwrap_err();
            assert!(error.to_string().starts_with(KEEP_DAYS), "{error}");
        }
    }

    #[test]
    fn the_longest_time_kept_still_reaches_back_from_now() {
        let longest = keeping(Some(&MAX_KEEP_DAYS.to_string())).unwrap();

        assert!(Clock::system().now().checked_sub(longest).is_ok());
    }

    #[test]
    fn shows_each_cohort_as_shares_of_its_size() {
        let asked = Asked::parse(&["--start", "signed_up", "--returning", "ordered"]).unwrap();
        let since: Timestamp = "2026-09-01T00:00:00Z".parse().unwrap();
        let cohorts = [Cohort {
            start: "2026-09-07".parse().unwrap(),
            size: 4,
            returned: vec![4, 1],
        }];

        assert_eq!(
            table(&asked, since, &cohorts),
            "who raised `ordered` again after `signed_up`, cohorts by week since 2026-09-01 (UTC)\n\
             cohort       size     0     1\n\
             2026-09-07      4  100%   25%\n"
        );
    }
}
