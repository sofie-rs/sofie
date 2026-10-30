use std::fmt;

use jiff::civil;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sofie_core::{Cast, ColumnReader, ColumnSpec, Error, Ordered, Result, Show, Shown, SqlValue};

#[cfg(feature = "web")]
use crate::Conventions;
use crate::{Input, Present};
use sofie_core::deserialize_parsed;

/// To the minute, as a time input posts it; stored as `HH:MM` text, which sorts as it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TimeOfDay {
    time: civil::Time,
}

impl TimeOfDay {
    pub fn try_new(hour: i8, minute: i8) -> Result<Self> {
        civil::Time::new(hour, minute, 0, 0)
            .map(|time| Self { time })
            .map_err(|_| invalid_time())
    }

    pub fn parse(text: &str) -> Result<Self> {
        let (hour, minute) = read(text.as_bytes()).ok_or_else(invalid_time)?;
        Ok(Self::at(hour, minute))
    }

    /// For a time written in code, such as a schedule's: a wrong one fails the build in a constant.
    ///
    /// ```compile_fail,E0080
    /// const _: () = { let _ = sofie_values::TimeOfDay::constant("25:00"); };
    /// ```
    #[must_use]
    pub const fn constant(text: &str) -> Self {
        match read(text.as_bytes()) {
            Some((hour, minute)) => Self::at(hour, minute),
            None => panic!("a time of day is written HH:MM, from 00:00 to 23:59"),
        }
    }

    const fn at(hour: i8, minute: i8) -> Self {
        Self {
            time: civil::Time::constant(hour, minute, 0, 0),
        }
    }

    #[must_use]
    pub fn hour(self) -> i8 {
        self.time.hour()
    }

    #[must_use]
    pub fn minute(self) -> i8 {
        self.time.minute()
    }

    #[must_use]
    pub fn civil(self) -> civil::Time {
        self.time
    }
}

const fn read(text: &[u8]) -> Option<(i8, i8)> {
    let [hour_tens, hour_ones, b':', minute_tens, minute_ones] = *text else {
        return None;
    };
    let (Some(hour), Some(minute)) = (
        two_digits(hour_tens, hour_ones),
        two_digits(minute_tens, minute_ones),
    ) else {
        return None;
    };
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((hour, minute))
}

const fn two_digits(tens: u8, ones: u8) -> Option<i8> {
    if !tens.is_ascii_digit() || !ones.is_ascii_digit() {
        return None;
    }
    #[expect(
        clippy::cast_possible_wrap,
        reason = "two decimal digits are at most 99"
    )]
    Some(((tens - b'0') * 10 + (ones - b'0')) as i8)
}

fn invalid_time() -> Error {
    Error::invalid("time_of_day", "must be a time such as 09:30")
}

impl fmt::Display for TimeOfDay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:02}:{:02}", self.hour(), self.minute())
    }
}

impl Ordered for TimeOfDay {}

impl Show for TimeOfDay {
    fn show(&self) -> Shown<'_> {
        Shown::Formatted(self)
    }
}

impl Cast for TimeOfDay {
    const COLUMNS: &'static [ColumnSpec] = String::COLUMNS;

    fn encode(&self, out: &mut Vec<SqlValue>) {
        out.push(SqlValue::Text(self.to_string()));
    }

    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        sofie_core::value::stored(Self::parse(row.text()?))
    }
}

impl Serialize for TimeOfDay {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TimeOfDay {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_parsed(deserializer, "a time of day", Self::parse)
    }
}

impl Present for TimeOfDay {
    const INPUT: Input = Input::typed(&[("type", "time")]);
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
    use super::*;
    use crate::testing::{assert_refused, round_trip};

    #[test]
    fn reads_exactly_hours_and_minutes() {
        let time = TimeOfDay::parse("09:05").unwrap();

        assert_eq!((time.hour(), time.minute()), (9, 5));
        assert_eq!(TimeOfDay::parse("23:59").unwrap().to_string(), "23:59");
        assert_eq!(TimeOfDay::try_new(9, 5).unwrap(), time);
        assert_eq!(
            TimeOfDay::constant("00:00"),
            TimeOfDay::try_new(0, 0).unwrap()
        );
        assert_refused(
            TimeOfDay::parse,
            &[
                "24:00", "12:60", "9:30", "09:30:00", "0930", " 09:30", "", "ab:cd",
            ],
        );
        assert!(TimeOfDay::try_new(24, 0).is_err());
    }

    #[test]
    #[should_panic(expected = "a time of day is written HH:MM")]
    fn a_wrong_constant_time_fails() {
        let _ = TimeOfDay::constant("25:00");
    }

    #[test]
    fn round_trips_through_a_text_column_and_json_and_sorts_as_it_reads() {
        let time = TimeOfDay::parse("07:45").unwrap();

        assert_eq!(round_trip(&time), vec![SqlValue::Text("07:45".into())]);
        let json = serde_json::to_value(time).unwrap();
        assert_eq!(json, serde_json::json!("07:45"));
        assert_eq!(serde_json::from_value::<TimeOfDay>(json).unwrap(), time);
        assert!(serde_json::from_value::<TimeOfDay>(serde_json::json!("7:45")).is_err());
        assert!(time < TimeOfDay::parse("10:00").unwrap());
        assert_eq!(TimeOfDay::INPUT.attributes, &[("type", "time")]);
    }
}
