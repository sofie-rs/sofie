use std::fmt;

use jiff::civil;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sofie_core::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimeOfDay {
    hour: i8,
    minute: i8,
}

impl TimeOfDay {
    pub fn new(hour: u8, minute: u8) -> Result<Self> {
        match (i8::try_from(hour), i8::try_from(minute)) {
            (Ok(hour @ 0..=23), Ok(minute @ 0..=59)) => Ok(Self { hour, minute }),
            _ => Err(refused()),
        }
    }

    pub fn parse(text: &str) -> Result<Self> {
        let (hour, minute) = text.split_once(':').ok_or_else(refused)?;
        let two_digits =
            |part: &str| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_digit());
        if !two_digits(hour) || !two_digits(minute) {
            return Err(refused());
        }
        Self::new(
            hour.parse().map_err(|_| refused())?,
            minute.parse().map_err(|_| refused())?,
        )
    }

    pub(crate) fn civil(self) -> civil::Time {
        civil::Time::constant(self.hour, self.minute, 0, 0)
    }

    pub(crate) fn of(time: civil::Time) -> Self {
        Self {
            hour: time.hour(),
            minute: time.minute(),
        }
    }
}

fn refused() -> Error {
    Error::invalid("time", "must be a time of day such as 22:00")
}

impl fmt::Display for TimeOfDay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:02}:{:02}", self.hour, self.minute)
    }
}

impl Serialize for TimeOfDay {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TimeOfDay {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(Error::into_deserialize_error)
    }
}

#[cfg(test)]
mod tests {
    use super::TimeOfDay;

    #[test]
    fn a_time_of_day_is_two_digit_hours_and_minutes() {
        assert_eq!(TimeOfDay::parse("07:05").unwrap().to_string(), "07:05");
        for refused in ["7:05", "+7:05", "07:+5", "24:00", "12:60", "12-00", "noon"] {
            assert!(TimeOfDay::parse(refused).is_err(), "{refused}");
        }
    }
}
