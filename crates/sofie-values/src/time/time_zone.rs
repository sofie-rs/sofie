use std::fmt;
use std::hash::{Hash, Hasher};

use jiff::civil;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sofie_core::{Cast, ColumnReader, ColumnSpec, Error, Result, Show, Shown, SqlValue};

use crate::parsed::deserialize_parsed;
use crate::{Date, Instant, TimeOfDay};

/// Stored by name, so a time kept in it follows the zone's rules as they change.
#[derive(Clone)]
pub struct TimeZone {
    /// Always one of the database's zones, which carry their IANA name.
    zone: jiff::tz::TimeZone,
}

impl TimeZone {
    pub fn parse(name: &str) -> Result<Self> {
        let zone = jiff::tz::TimeZone::get(name).map_err(|_| {
            Error::invalid(
                "time_zone",
                "must be an IANA time zone such as Europe/Istanbul",
            )
        })?;
        Ok(Self { zone })
    }

    #[must_use]
    pub fn name(&self) -> &str {
        self.zone
            .iana_name()
            .expect("a zone found in the database carries its name")
    }

    pub(crate) fn zoned(&self, instant: Instant) -> jiff::Zoned {
        instant.timestamp().to_zoned(self.zone.clone())
    }

    pub(crate) fn instant_of(&self, local: civil::DateTime) -> Result<Instant> {
        sofie_core::wall_clock_instant(&self.zone, local)
            .map(Instant::from)
            .ok_or_else(|| Error::internal_message(format!("placing {local} in {}", self.name())))
    }

    pub fn instant_at(&self, day: Date, time: TimeOfDay) -> Result<Instant> {
        self.instant_of(day.civil().to_datetime(time.civil()))
    }
}

impl PartialEq for TimeZone {
    fn eq(&self, other: &Self) -> bool {
        self.name() == other.name()
    }
}

impl Eq for TimeZone {}

impl Hash for TimeZone {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name().hash(state);
    }
}

impl fmt::Debug for TimeZone {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "TimeZone({})", self.name())
    }
}

impl fmt::Display for TimeZone {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl Show for TimeZone {
    fn show(&self) -> Shown<'_> {
        Shown::Text(self.name())
    }
}

/// Parsed from the stored text in place, so reading a zone allocates no string.
impl Cast for TimeZone {
    const COLUMNS: &'static [ColumnSpec] = String::COLUMNS;

    fn encode(&self, out: &mut Vec<SqlValue>) {
        out.push(SqlValue::Text(self.name().to_owned()));
    }

    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        sofie_core::value::stored(Self::parse(row.text()?))
    }
}

impl Serialize for TimeZone {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for TimeZone {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_parsed(deserializer, "an IANA time zone name", Self::parse)
    }
}

crate::present::present_text!(TimeZone, crate::present::TEXT, Exact, |zone| zone
    .name()
    .to_owned());

#[cfg(test)]
mod tests {
    use sofie_core::{Cast, RowReader, SqlValue};

    use super::*;
    use crate::testing::{assert_refused, round_trip};

    #[test]
    fn accepts_iana_names_in_their_own_spelling() {
        assert_eq!(
            TimeZone::parse("europe/istanbul").unwrap().name(),
            "Europe/Istanbul"
        );
        assert_eq!(TimeZone::parse("UTC").unwrap().to_string(), "UTC");
    }

    #[test]
    fn refuses_what_is_not_a_zone_name() {
        assert_refused(
            TimeZone::parse,
            &["", "Europe/Atlantis", "+03:00", "Istanbul Time"],
        );
    }

    #[test]
    fn round_trips_through_a_text_column_and_json() {
        let zone = TimeZone::parse("America/New_York").unwrap();
        assert_eq!(
            round_trip(&zone),
            vec![SqlValue::Text("America/New_York".into())]
        );
        let json = serde_json::to_value(&zone).unwrap();
        assert_eq!(serde_json::from_value::<TimeZone>(json).unwrap(), zone);
        assert_eq!(format!("{zone:?}"), "TimeZone(America/New_York)");
    }

    fn placed(zone: &str, day: &str, time: &str) -> String {
        TimeZone::parse(zone)
            .unwrap()
            .instant_at(Date::parse(day).unwrap(), TimeOfDay::parse(time).unwrap())
            .unwrap()
            .to_string()
    }

    #[test]
    fn a_skipped_time_is_placed_where_the_shift_ends_and_a_repeated_one_first() {
        assert_eq!(
            placed("Europe/Istanbul", "2026-10-25", "02:30"),
            "2026-10-24T23:30:00Z"
        );
        assert_eq!(
            placed("Europe/Berlin", "2026-03-29", "02:30"),
            "2026-03-29T01:00:00Z"
        );
        assert_eq!(
            placed("Europe/Berlin", "2026-10-25", "02:30"),
            "2026-10-25T00:30:00Z"
        );
        assert_eq!(
            placed("Europe/Berlin", "2026-10-25", "03:00"),
            "2026-10-25T02:00:00Z"
        );
    }

    #[test]
    fn a_stored_name_that_is_no_zone_is_an_internal_error() {
        let values = [SqlValue::Text("Europe/Atlantis".into())];

        assert!(matches!(
            TimeZone::decode(&mut RowReader::new(&values)),
            Err(Error::Internal { .. })
        ));
    }
}
