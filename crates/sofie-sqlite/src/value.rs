//! How each value is stored: SQLite keeps integers, reals, text and blobs, so a uuid
//! is its text, a timestamp its microseconds since 1970, a date and a date-time the
//! text that sorts them, a boolean 0 or 1 and JSON its text.

use rusqlite::types::Value;
use sofie_core::{Date, DateTime, Error, Result, SqlValue, Timestamp};

const DATE_TIME: &str = "%Y-%m-%dT%H:%M:%S%.6f";

pub(crate) fn stored(value: &SqlValue) -> Result<Value> {
    Ok(match value {
        SqlValue::Null => Value::Null,
        SqlValue::Bool(flag) => Value::Integer(i64::from(*flag)),
        SqlValue::Int(number) => Value::Integer(*number),
        SqlValue::Float(number) => Value::Real(*number),
        SqlValue::Text(text) => Value::Text(text.clone()),
        SqlValue::Bytes(bytes) => Value::Blob(bytes.clone()),
        SqlValue::Uuid(uuid) => Value::Text(uuid.to_string()),
        SqlValue::Timestamp(at) => Value::Integer(at.as_microsecond()),
        SqlValue::Date(date) => Value::Text(date.to_string()),
        SqlValue::DateTime(at) => Value::Text(at.strftime(DATE_TIME).to_string()),
        SqlValue::Json(json) => Value::Text(json.to_string()),
        SqlValue::Array(_) => {
            return Err(Error::internal_message(
                "SQLite binds no arrays; the dialect writes each value as its own parameter",
            ));
        }
    })
}

pub(crate) fn timestamp(micros: i64) -> Result<Timestamp> {
    Timestamp::from_microsecond(micros)
        .map_err(|error| Error::internal("reading a timestamp from microseconds", error))
}

pub(crate) fn date(text: &str) -> Result<Date> {
    text.parse()
        .map_err(|error| Error::internal("reading stored text as a date", error))
}

pub(crate) fn date_time(text: &str) -> Result<DateTime> {
    text.parse()
        .map_err(|error| Error::internal("reading stored text as a date-time", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timestamp_is_kept_to_the_microsecond_and_a_date_time_as_sortable_text() {
        let at: Timestamp = "2026-10-14T09:30:00.123456Z".parse().unwrap();
        let local: DateTime = "2026-10-14T09:30:00".parse().unwrap();

        let Value::Integer(stored_at) = stored(&SqlValue::Timestamp(at)).unwrap() else {
            panic!("a timestamp is an integer");
        };
        assert_eq!(timestamp(stored_at).unwrap(), at);
        assert_eq!(
            stored(&SqlValue::DateTime(local)).unwrap(),
            Value::Text("2026-10-14T09:30:00.000000".to_owned())
        );
        assert_eq!(date_time("2026-10-14T09:30:00.000000").unwrap(), local);
    }

    #[test]
    fn a_value_that_cannot_be_read_stays_out_of_the_error() {
        let errors = [
            format!("{:?}", date("ada@example.com").unwrap_err()),
            format!("{:?}", date_time("ada@example.com").unwrap_err()),
        ];

        for error in errors {
            assert!(!error.contains("ada@example.com"), "{error}");
        }
    }
}
