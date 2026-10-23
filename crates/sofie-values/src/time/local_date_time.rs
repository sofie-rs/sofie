use jiff::civil;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sofie_core::show::Fields;
use sofie_core::{Cast, ColumnReader, ColumnSpec, Error, Result, Show, Shown, SqlType, SqlValue};

use crate::{Control, Input, Instant, Part, Present, TimeZone};

/// The instant is worked out when needed, so the time survives a change to the zone's rules.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LocalDateTime {
    local: civil::DateTime,
    zone: TimeZone,
}

impl LocalDateTime {
    #[must_use]
    pub fn new(local: civil::DateTime, zone: TimeZone) -> Self {
        Self { local, zone }
    }

    pub fn parse(local: &str, zone: TimeZone) -> Result<Self> {
        Ok(Self::new(parse_local(local)?, zone))
    }

    #[must_use]
    pub fn local(&self) -> civil::DateTime {
        self.local
    }

    #[must_use]
    pub fn zone(&self) -> &TimeZone {
        &self.zone
    }

    pub fn instant(&self) -> Result<Instant> {
        self.zone.instant_of(self.local)
    }
}

fn parse_local(local: &str) -> Result<civil::DateTime> {
    let refusal = || {
        Error::invalid(
            "local",
            "must be a date and time without an offset, such as 2026-11-01T09:30",
        )
    };
    if names_an_offset(local) {
        return Err(refusal());
    }
    local.parse().map_err(|_| refusal())
}

/// The parser would ignore an offset, silently changing what the text means.
fn names_an_offset(local: &str) -> bool {
    local
        .split_once(['T', 't', ' '])
        .is_some_and(|(_, time)| time.contains(['+', '-', 'Z', 'z', '[']))
}

/// ISO 8601 text, which sorts as it reads: the value model has no zone-less time type.
impl Show for LocalDateTime {
    fn show(&self) -> Shown<'_> {
        Shown::Record(self)
    }
}

impl Fields for LocalDateTime {
    fn name(&self, index: usize) -> Option<&str> {
        ["local", "zone"].get(index).copied()
    }

    fn get(&self, name: &str) -> Option<Shown<'_>> {
        match name {
            "local" => Some(Shown::Formatted(&self.local)),
            "zone" => Some(self.zone.show()),
            _ => None,
        }
    }
}

impl Cast for LocalDateTime {
    const COLUMNS: &'static [ColumnSpec] = &[
        ColumnSpec::new("", SqlType::DateTime),
        ColumnSpec::new("_zone", SqlType::Text).unordered(),
    ];

    fn encode(&self, out: &mut Vec<SqlValue>) {
        out.push(SqlValue::DateTime(self.local));
        self.zone.encode(out);
    }

    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        let local = row.date_time()?;
        Ok(Self::new(local, TimeZone::decode(row)?))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalDateTimeFields {
    local: String,
    zone: TimeZone,
}

impl Serialize for LocalDateTime {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let fields = LocalDateTimeFields {
            local: self.local.to_string(),
            zone: self.zone.clone(),
        };
        fields.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for LocalDateTime {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = LocalDateTimeFields::deserialize(deserializer)?;
        Self::parse(&fields.local, fields.zone).map_err(Error::into_deserialize_error)
    }
}

/// Sorted by its wall-clock time: an instant may not exist for it.
impl Present for LocalDateTime {
    const INPUT: Input = Input::new(Control::Parts(&[
        Part {
            name: "local",
            attributes: &[("type", "datetime-local")],
            options: &[],
        },
        Part {
            name: "zone",
            attributes: &[("type", "text")],
            options: &[],
        },
    ]));
    const SEARCH: crate::Search = crate::Search::Time;

    fn csv(&self) -> String {
        format!("{} {}", self.local, self.zone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{round_trip, suffixes};

    fn zone(name: &str) -> TimeZone {
        TimeZone::parse(name).unwrap()
    }

    fn meeting(local: &str, name: &str) -> LocalDateTime {
        LocalDateTime::parse(local, zone(name)).unwrap()
    }

    #[test]
    fn works_out_its_instant_in_its_zone() {
        let at = meeting("2026-11-01T09:30", "Europe/Istanbul");

        assert_eq!(at.instant().unwrap().to_string(), "2026-11-01T06:30:00Z");
        assert_eq!(at.local().to_string(), "2026-11-01T09:30:00");
        assert_eq!(at.zone().name(), "Europe/Istanbul");
    }

    #[test]
    fn places_a_skipped_time_where_the_shift_ends_and_a_repeated_one_first() {
        let skipped = meeting("2026-03-08T02:30", "America/New_York");
        let repeated = meeting("2026-11-01T01:30", "America/New_York");

        assert_eq!(
            skipped.instant().unwrap().to_string(),
            "2026-03-08T07:00:00Z"
        );
        assert_eq!(
            repeated.instant().unwrap().to_string(),
            "2026-11-01T05:30:00Z"
        );
    }

    #[test]
    fn refuses_a_time_with_an_offset_or_none_at_all() {
        for local in [
            "2026-11-01T09:30:00Z",
            "2026-11-01T09:30+03:00",
            "2026-11-01 09:30-05:00",
            "tomorrow",
            "",
        ] {
            assert!(
                LocalDateTime::parse(local, zone("UTC")).is_err(),
                "{local:?}"
            );
        }
    }

    #[test]
    fn round_trips_through_a_time_and_a_zone_column() {
        let at = meeting("2026-11-01T09:30", "Europe/Istanbul");
        assert_eq!(
            round_trip(&at),
            vec![
                SqlValue::DateTime(civil::date(2026, 11, 1).at(9, 30, 0, 0)),
                SqlValue::Text("Europe/Istanbul".into()),
            ]
        );
        assert_eq!(suffixes::<LocalDateTime>(), ["", "_zone"]);
    }

    #[test]
    fn round_trips_through_json() {
        let at = meeting("2026-11-01T09:30", "Europe/Istanbul");
        let json = serde_json::to_value(&at).unwrap();

        assert_eq!(
            json,
            serde_json::json!({"local": "2026-11-01T09:30:00", "zone": "Europe/Istanbul"})
        );
        assert_eq!(serde_json::from_value::<LocalDateTime>(json).unwrap(), at);
        let unknown_zone = serde_json::json!({"local": "2026-11-01T09:30", "zone": "Mars/Base"});
        assert!(serde_json::from_value::<LocalDateTime>(unknown_zone).is_err());
    }

    #[test]
    fn builds_from_a_civil_time() {
        let local = civil::date(2026, 11, 1).at(9, 30, 0, 0);

        assert_eq!(
            LocalDateTime::new(local, zone("Europe/Istanbul")),
            meeting("2026-11-01T09:30", "Europe/Istanbul")
        );
    }
}
