use crate::{Control, Input, Part, Present};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sofie_core::show::Fields;
use sofie_core::{Cast, ColumnReader, ColumnSpec, Error, Result, Show, Shown, SqlType, SqlValue};

/// About a centimetre: far below what a float column or JSON number loses, so it round-trips.
const STEPS_PER_DEGREE: f64 = 1e7;
const MAXIMUM_LATITUDE: f64 = 90.0;
const MAXIMUM_LONGITUDE: f64 = 180.0;
const DEGREES: &[(&str, &str)] = &[("type", "number"), ("step", "any")];

/// Whole ten-millionths of a degree, so two equal points compare equal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Coordinate {
    latitude: i32,
    longitude: i32,
}

impl Coordinate {
    pub fn try_new(latitude: f64, longitude: f64) -> Result<Self> {
        Ok(Self {
            latitude: steps("latitude", latitude, MAXIMUM_LATITUDE)?,
            longitude: steps("longitude", longitude, MAXIMUM_LONGITUDE)?,
        })
    }

    #[must_use]
    pub fn latitude(self) -> f64 {
        f64::from(self.latitude) / STEPS_PER_DEGREE
    }

    #[must_use]
    pub fn longitude(self) -> f64 {
        f64::from(self.longitude) / STEPS_PER_DEGREE
    }
}

fn steps(field: &str, degrees: f64, maximum: f64) -> Result<i32> {
    if !degrees.is_finite() || degrees.abs() > maximum {
        return Err(Error::invalid(
            field,
            format!("must be a number of degrees from -{maximum} to {maximum}"),
        ));
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "at most 180 degrees is 1.8e9 steps, inside i32"
    )]
    let steps = (degrees * STEPS_PER_DEGREE).round() as i32;
    Ok(steps)
}

impl Show for Coordinate {
    fn show(&self) -> Shown<'_> {
        Shown::Record(self)
    }
}

impl Fields for Coordinate {
    fn name(&self, index: usize) -> Option<&str> {
        ["latitude", "longitude"].get(index).copied()
    }

    fn get(&self, name: &str) -> Option<Shown<'_>> {
        match name {
            "latitude" => Some(Shown::Float(self.latitude())),
            "longitude" => Some(Shown::Float(self.longitude())),
            _ => None,
        }
    }
}

impl Cast for Coordinate {
    const COLUMNS: &'static [ColumnSpec] = &[
        ColumnSpec::new("_latitude", SqlType::Float).unordered(),
        ColumnSpec::new("_longitude", SqlType::Float).unordered(),
    ];

    fn encode(&self, out: &mut Vec<SqlValue>) {
        out.push(SqlValue::Float(self.latitude()));
        out.push(SqlValue::Float(self.longitude()));
    }

    fn decode(row: &mut dyn ColumnReader) -> Result<Self> {
        let latitude = f64::decode(row)?;
        let longitude = f64::decode(row)?;
        Self::try_new(latitude, longitude)
            .map_err(|error| Error::internal("a stored coordinate is not valid", error))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoordinateFields {
    #[serde(deserialize_with = "degrees")]
    latitude: f64,
    #[serde(deserialize_with = "degrees")]
    longitude: f64,
}

/// A form posts each part as text.
fn degrees<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Degrees {
        Number(f64),
        Text(String),
    }
    match Degrees::deserialize(deserializer)? {
        Degrees::Number(degrees) => Ok(degrees),
        Degrees::Text(text) => text
            .trim()
            .parse()
            .map_err(|_| serde::de::Error::custom("must be a number of degrees")),
    }
}

impl Serialize for Coordinate {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let fields = CoordinateFields {
            latitude: self.latitude(),
            longitude: self.longitude(),
        };
        fields.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Coordinate {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = CoordinateFields::deserialize(deserializer)?;
        Self::try_new(fields.latitude, fields.longitude).map_err(Error::into_deserialize_error)
    }
}

impl Present for Coordinate {
    const INPUT: Input = Input::new(Control::Parts(&[
        Part {
            name: "latitude",
            attributes: DEGREES,
            options: &[],
        },
        Part {
            name: "longitude",
            attributes: DEGREES,
            options: &[],
        },
    ]));
    const SEARCH: crate::Search = crate::Search::Skipped;

    fn csv(&self) -> String {
        format!("{},{}", self.latitude(), self.longitude())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{round_trip, suffixes};

    #[test]
    fn accepts_the_poles_and_the_antimeridian() {
        for (latitude, longitude) in [(90.0, 180.0), (-90.0, -180.0), (0.0, 0.0)] {
            let point = Coordinate::try_new(latitude, longitude).unwrap();
            assert_eq!((point.latitude(), point.longitude()), (latitude, longitude));
        }
    }

    #[test]
    fn refuses_degrees_off_the_globe() {
        for (latitude, longitude) in [
            (90.000_000_1, 0.0),
            (0.0, -180.000_000_1),
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
        ] {
            assert!(
                matches!(
                    Coordinate::try_new(latitude, longitude),
                    Err(Error::Invalid(_))
                ),
                "{latitude}, {longitude}"
            );
        }
    }

    #[test]
    fn equal_points_compare_equal_below_the_precision() {
        assert_eq!(
            Coordinate::try_new(41.008_238_01, 28.978_359).unwrap(),
            Coordinate::try_new(41.008_238, 28.978_359_04).unwrap()
        );
    }

    #[test]
    fn round_trips_through_two_float_columns() {
        let point = Coordinate::try_new(41.008_238_1, 28.978_359_3).unwrap();
        assert_eq!(
            round_trip(&point),
            vec![SqlValue::Float(41.008_238_1), SqlValue::Float(28.978_359_3)]
        );
        assert_eq!(suffixes::<Coordinate>(), ["_latitude", "_longitude"]);
    }

    #[test]
    fn round_trips_through_json_as_numbers() {
        let point = Coordinate::try_new(41.008_238, -28.978_359).unwrap();
        let json = serde_json::to_value(point).unwrap();

        assert_eq!(
            json,
            serde_json::json!({"latitude": 41.008_238, "longitude": -28.978_359})
        );
        assert_eq!(serde_json::from_value::<Coordinate>(json).unwrap(), point);
        let off = serde_json::json!({"latitude": 91, "longitude": 0});
        assert!(serde_json::from_value::<Coordinate>(off).is_err());
    }
}
