use sofie_core::{Error, Instant, Result, Timestamp};
use sofie_values::{Date, TimeOfDay, TimeZone, Weekday};

/// `#[every(daily_at = "03:00", zone = "Europe/Istanbul")]`; a worker refuses an unknown zone at start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WallClock {
    at: TimeOfDay,
    on: Option<Weekday>,
    zone: &'static str,
}

const DAYS_LOOKED_AHEAD: usize = 8;

impl WallClock {
    #[must_use]
    pub const fn daily(at: TimeOfDay, zone: &'static str) -> Self {
        Self { at, on: None, zone }
    }

    #[must_use]
    pub const fn weekly(on: Weekday, at: TimeOfDay, zone: &'static str) -> Self {
        Self {
            at,
            on: Some(on),
            zone,
        }
    }

    pub(crate) fn zone(self) -> Result<TimeZone> {
        TimeZone::parse(self.zone).map_err(|_| {
            Error::internal_message(format!(
                "`{}` is not an IANA time zone such as Europe/Istanbul",
                self.zone
            ))
        })
    }

    /// Counted in the zone's days, so a time its clocks show twice runs once, at the first.
    pub(crate) fn next_after(self, now: Timestamp) -> Result<Timestamp> {
        let zone = self.zone()?;
        let mut day = Date::of(Instant::from(now), &zone);
        for _ in 0..DAYS_LOOKED_AHEAD {
            if self.on.is_none_or(|weekday| day.weekday() == weekday) {
                let run = zone.instant_at(day, self.at)?.timestamp();
                if run > now {
                    return Ok(run);
                }
            }
            day = day
                .civil()
                .tomorrow()
                .map(Date::from)
                .map_err(|error| Error::internal("finding a schedule's next day", error))?;
        }
        Err(Error::internal_message(format!(
            "no run of {self:?} within a week after {now}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next(wall_clock: WallClock, now: &str) -> String {
        wall_clock
            .next_after(now.parse().unwrap())
            .unwrap()
            .to_string()
    }

    fn daily(at: &str, zone: &'static str) -> WallClock {
        WallClock::daily(TimeOfDay::parse(at).unwrap(), zone)
    }

    #[test]
    fn a_daily_run_is_today_until_it_passes_then_tomorrow() {
        let three = daily("03:00", "Europe/Istanbul");

        assert_eq!(next(three, "2026-10-20T23:59:00Z"), "2026-10-21T00:00:00Z");
        assert_eq!(next(three, "2026-10-21T00:00:00Z"), "2026-10-22T00:00:00Z");
    }

    #[test]
    fn a_skipped_time_runs_where_the_shift_ends() {
        let half_past_two = daily("02:30", "Europe/Berlin");

        assert_eq!(
            next(half_past_two, "2026-03-28T12:00:00Z"),
            "2026-03-29T01:00:00Z"
        );
        assert_eq!(
            next(half_past_two, "2026-03-29T01:00:00Z"),
            "2026-03-30T00:30:00Z"
        );
    }

    #[test]
    fn a_repeated_time_runs_once_at_its_first_occurrence() {
        let half_past_two = daily("02:30", "Europe/Berlin");

        assert_eq!(
            next(half_past_two, "2026-10-24T12:00:00Z"),
            "2026-10-25T00:30:00Z"
        );
        assert_eq!(
            next(half_past_two, "2026-10-25T00:30:00Z"),
            "2026-10-26T01:30:00Z"
        );
        assert_eq!(
            next(half_past_two, "2026-10-25T01:30:00Z"),
            "2026-10-26T01:30:00Z"
        );
    }

    #[test]
    fn a_weekly_run_waits_for_its_day() {
        let monday = WallClock::weekly(
            Weekday::Monday,
            TimeOfDay::constant("09:00"),
            "Europe/Berlin",
        );

        assert_eq!(next(monday, "2026-10-21T12:00:00Z"), "2026-10-26T08:00:00Z");
        assert_eq!(next(monday, "2026-10-26T07:59:00Z"), "2026-10-26T08:00:00Z");
        assert_eq!(next(monday, "2026-10-26T08:00:00Z"), "2026-11-02T08:00:00Z");
    }

    #[test]
    fn an_unknown_zone_is_refused() {
        let error = daily("03:00", "Mars/Olympus")
            .next_after("2026-10-21T00:00:00Z".parse().unwrap())
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("`Mars/Olympus` is not an IANA time zone"),
            "{error}"
        );
    }
}
