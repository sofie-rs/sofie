use std::convert::Infallible;
use std::fmt;
use std::time::Duration;

use jiff::Timestamp;
use jiff::civil::DateTime;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::value::Value;
use crate::{DisplayZone, Error, FieldError, Ordered, Result};

/// Stored and written in UTC whatever offset it was read with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Instant {
    timestamp: Timestamp,
}

impl Instant {
    /// RFC 3339 with an offset, or a `datetime-local` value without one, read in the display zone.
    pub fn parse(text: &str) -> Result<Self> {
        Self::parse_in(text, DisplayZone::current())
    }

    pub fn parse_in(text: &str, zone: &DisplayZone) -> Result<Self> {
        let text = text.trim();
        if Self::is_wall_clock(text)
            && let Ok(wall_clock) = text.parse::<DateTime>()
        {
            return zone.instant_at(wall_clock);
        }
        text.parse::<Timestamp>().map(Self::from).map_err(|_| {
            Error::Invalid(vec![FieldError::new(
                "instant",
                "validation.instant.invalid",
                "must be a time with an offset, such as 2026-10-11T09:30:00Z",
            )])
        })
    }

    /// A `datetime-local` value: minutes, or seconds and a fraction when the control's `step` allows.
    #[must_use]
    pub fn is_wall_clock(text: &str) -> bool {
        let (clock, fraction) = text.split_once('.').unwrap_or((text, ""));
        let fraction_fits = clock.len() == text.len()
            || (clock.len() == 19
                && !fraction.is_empty()
                && fraction.bytes().all(|byte| byte.is_ascii_digit()));
        fraction_fits
            && matches!(
                (clock.len(), clock.as_bytes().get(10)),
                (16 | 19, Some(b'T'))
            )
            && clock
                .bytes()
                .skip(11)
                .all(|byte| byte.is_ascii_digit() || byte == b':')
    }

    #[must_use]
    pub fn timestamp(self) -> Timestamp {
        self.timestamp
    }

    pub fn checked_add(self, duration: Duration) -> Result<Self> {
        self.timestamp
            .checked_add(duration)
            .map(Self::from)
            .map_err(|error| Error::internal("adding to an instant", error))
    }

    pub fn checked_sub(self, duration: Duration) -> Result<Self> {
        self.timestamp
            .checked_sub(duration)
            .map(Self::from)
            .map_err(|error| Error::internal("subtracting from an instant", error))
    }
}

impl From<Timestamp> for Instant {
    fn from(timestamp: Timestamp) -> Self {
        let timestamp = Timestamp::from_microsecond(timestamp.as_microsecond()).expect(
            "a timestamp truncated to the microseconds both databases store stays in range",
        );
        Self { timestamp }
    }
}

impl fmt::Display for Instant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.timestamp, formatter)
    }
}

impl Value for Instant {
    type Inner = Timestamp;

    fn inner(&self) -> &Timestamp {
        &self.timestamp
    }

    fn try_from_inner(timestamp: Timestamp) -> Result<Self, impl fmt::Display> {
        Ok::<_, Infallible>(Self::from(timestamp))
    }
}

impl Ordered for Instant {}

impl Serialize for Instant {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Instant {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(Error::into_deserialize_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cast, RowReader, SqlValue};

    fn instant(text: &str) -> Instant {
        Instant::parse(text).unwrap()
    }

    #[test]
    fn reads_any_offset_and_writes_utc() {
        assert_eq!(
            instant("2026-10-11T12:30:00+03:00").to_string(),
            "2026-10-11T09:30:00Z"
        );
        assert_eq!(
            instant("2026-10-11T09:30:00Z"),
            instant("2026-10-11T12:30:00+03:00")
        );
        assert!(instant("2026-10-11T09:30:00Z") < instant("2026-10-11T09:30:01Z"));
    }

    #[test]
    fn reads_a_date_time_control_s_minute_or_second_in_the_display_zone() {
        let istanbul = DisplayZone::named("Europe/Istanbul").unwrap();
        let read = |text| Instant::parse_in(text, &istanbul).unwrap();

        assert_eq!(read("2026-10-11T12:30"), instant("2026-10-11T09:30:00Z"));
        assert_eq!(read("2026-10-11T12:30:15"), instant("2026-10-11T09:30:15Z"));
        assert_eq!(
            read("2026-10-11T12:30:15.25"),
            instant("2026-10-11T09:30:15.25Z")
        );
        assert_eq!(
            read("2026-10-11T12:30:00Z"),
            instant("2026-10-11T12:30:00Z")
        );
        assert_eq!(instant("2026-10-11T09:30"), instant("2026-10-11T09:30:00Z"));
    }

    #[test]
    fn reads_a_wall_clock_skipped_by_a_forward_shift_where_the_shift_ends() {
        let berlin = DisplayZone::named("Europe/Berlin").unwrap();

        let read = Instant::parse_in("2026-03-29T02:30", &berlin).unwrap();

        assert_eq!(read, instant("2026-03-29T01:00:00Z"));
    }

    #[test]
    fn refuses_a_repeated_wall_clock_naming_both_offsets() {
        let berlin = DisplayZone::named("Europe/Berlin").unwrap();

        let error = Instant::parse_in("2026-10-25T02:30", &berlin).unwrap_err();

        let Error::Invalid(errors) = &error else {
            panic!("{error}");
        };
        let message = &errors[0].message;
        assert!(
            message.contains("+02:00") && message.contains("+01:00"),
            "{message}"
        );
        assert!(message.contains("2026-10-25T02:30:00+02:00"), "{message}");
        assert_eq!(
            Instant::parse_in("2026-10-25T02:30:00+01:00", &berlin).unwrap(),
            instant("2026-10-25T01:30:00Z")
        );
    }

    #[test]
    fn never_reads_a_short_offset_as_a_wall_clock() {
        assert_ne!(
            Instant::parse("2026-10-11T09:30+03").ok(),
            Some(instant("2026-10-11T09:30:00Z"))
        );
    }

    #[test]
    fn refuses_what_is_neither_an_offset_time_nor_a_date_time_control_s_value() {
        for text in ["2026-10-11 09:30:00", "2026-10-11", "yesterday", ""] {
            assert!(Instant::parse(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn round_trips_through_a_timestamp_column_and_json() {
        let moment = instant("2026-10-11T09:30:00.5Z");
        let mut values = Vec::new();
        moment.encode(&mut values);

        assert_eq!(values, vec![SqlValue::Timestamp(moment.timestamp())]);
        assert_eq!(
            Instant::decode(&mut RowReader::new(&values)).unwrap(),
            moment
        );
        let json = serde_json::to_value(moment).unwrap();
        assert_eq!(json, serde_json::json!("2026-10-11T09:30:00.5Z"));
        assert_eq!(serde_json::from_value::<Instant>(json).unwrap(), moment);
        assert!(serde_json::from_value::<Instant>(serde_json::json!(0)).is_err());
    }

    #[test]
    fn keeps_the_microseconds_a_database_stores() {
        assert_eq!(
            instant("2026-10-11T09:30:00.123456789Z"),
            instant("2026-10-11T09:30:00.123456Z")
        );
        let before_1970 = Timestamp::new(-5, -123_456_789).unwrap();
        assert_eq!(
            Instant::from(before_1970).timestamp(),
            Timestamp::new(-5, -123_456_000).unwrap()
        );
    }

    #[test]
    fn moves_by_a_duration() {
        let moment = instant("2026-10-11T09:30:00Z");

        assert_eq!(
            moment.checked_add(Duration::from_secs(60)).unwrap(),
            instant("2026-10-11T09:31:00Z")
        );
        assert_eq!(
            moment.checked_sub(Duration::from_secs(60)).unwrap(),
            instant("2026-10-11T09:29:00Z")
        );
    }
}
