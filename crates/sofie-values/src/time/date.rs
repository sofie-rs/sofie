use std::convert::Infallible;
use std::fmt;

use jiff::civil;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sofie_core::value::Value;
use sofie_core::{Error, Ordered, Result};

#[cfg(feature = "web")]
use crate::Conventions;
use crate::parsed::deserialize_parsed;
use crate::{Input, Instant, Present, TimeZone, Weekday};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Date {
    day: civil::Date,
}

impl Date {
    pub fn try_new(year: i16, month: i8, day: i8) -> Result<Self> {
        civil::Date::new(year, month, day)
            .map(Self::from)
            .map_err(|_| invalid_date())
    }

    pub fn parse(text: &str) -> Result<Self> {
        let day: civil::Date = text.parse().map_err(|_| invalid_date())?;
        if day.to_string() != text {
            return Err(invalid_date());
        }
        Ok(Self::from(day))
    }

    /// Today's date is `Date::utc_of(clock.now())`.
    #[must_use]
    pub fn utc_of(instant: Instant) -> Self {
        Self::from(instant.timestamp().to_zoned(jiff::tz::TimeZone::UTC).date())
    }

    /// The day `instant` falls on where `zone`'s clocks are.
    #[must_use]
    pub fn of(instant: Instant, zone: &TimeZone) -> Self {
        Self::from(zone.zoned(instant).date())
    }

    #[must_use]
    pub fn civil(self) -> civil::Date {
        self.day
    }

    #[must_use]
    pub fn weekday(self) -> Weekday {
        Weekday::from(self.day.weekday())
    }
}

fn invalid_date() -> Error {
    Error::invalid("date", "must be a calendar day such as 2026-10-11")
}

impl From<civil::Date> for Date {
    fn from(day: civil::Date) -> Self {
        Self { day }
    }
}

impl fmt::Display for Date {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.day, formatter)
    }
}

impl Value for Date {
    type Inner = civil::Date;

    fn inner(&self) -> &civil::Date {
        &self.day
    }

    fn try_from_inner(day: civil::Date) -> Result<Self, impl fmt::Display> {
        Ok::<_, Infallible>(Self::from(day))
    }
}

impl Ordered for Date {}

impl Serialize for Date {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Date {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_parsed(deserializer, "a calendar day", Self::parse)
    }
}

impl Present for Date {
    const INPUT: Input = Input::typed(&[("type", "date")]);
    const SEARCH: crate::Search = crate::Search::Time;

    fn csv(&self) -> String {
        self.to_string()
    }

    #[cfg(feature = "web")]
    fn write_localized(&self, out: &mut String, conventions: &dyn Conventions) {
        conventions.write_date(out, &self.csv());
    }
}

#[cfg(test)]
mod tests {
    use sofie_core::SqlValue;

    use super::*;
    use crate::testing::round_trip;

    #[test]
    fn an_instant_falls_on_its_utc_day_and_on_its_zone_s_local_day() {
        let late_evening = Instant::parse("2026-10-11T22:30:00Z").unwrap();
        let istanbul = TimeZone::parse("Europe/Istanbul").unwrap();

        assert_eq!(
            Date::utc_of(late_evening),
            Date::parse("2026-10-11").unwrap()
        );
        assert_eq!(
            Date::of(late_evening, &istanbul),
            Date::parse("2026-10-12").unwrap()
        );
    }

    #[test]
    fn accepts_real_days_only() {
        assert_eq!(
            Date::try_new(2024, 2, 29).unwrap(),
            Date::parse("2024-02-29").unwrap()
        );
        assert!(Date::try_new(2026, 2, 29).is_err());
        assert!(Date::try_new(2026, 13, 1).is_err());
        assert!(Date::try_new(2026, 1, 0).is_err());
    }

    #[test]
    fn reads_exactly_year_month_day() {
        for text in [
            "2026-10-11T09:30",
            "2026-1-1",
            "20261011",
            " 2026-10-11",
            "2026-10-32",
            "",
        ] {
            assert!(Date::parse(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn round_trips_through_a_date_column_and_json() {
        let day = Date::try_new(2026, 10, 11).unwrap();
        assert_eq!(round_trip(&day), vec![SqlValue::Date(day.civil())]);
        let json = serde_json::to_value(day).unwrap();
        assert_eq!(json, serde_json::json!("2026-10-11"));
        assert_eq!(serde_json::from_value::<Date>(json).unwrap(), day);
        assert!(day < Date::try_new(2026, 10, 12).unwrap());
    }
}
