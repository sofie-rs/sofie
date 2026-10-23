use std::fmt::Debug;

use sofie_core::{Cast, Error, Result, RowReader, SqlValue};

use crate::{Currency, Money};

pub(crate) fn euros(minor: i64) -> Money {
    Money::new(minor, Currency::parse("EUR").unwrap())
}

/// Returns the columns the value is stored in, once it read back from them unchanged.
pub(crate) fn round_trip<T: Cast + PartialEq + Debug>(value: &T) -> Vec<SqlValue> {
    let mut values = Vec::new();
    value.encode(&mut values);
    assert_eq!(&T::decode(&mut RowReader::new(&values)).unwrap(), value);
    values
}

pub(crate) fn suffixes<T: Cast>() -> Vec<&'static str> {
    T::COLUMNS.iter().map(|column| column.suffix).collect()
}

pub(crate) fn assert_refused<T>(parse: fn(&str) -> Result<T>, texts: &[&str]) {
    for text in texts {
        assert!(
            matches!(parse(text), Err(Error::Invalid(_))),
            "{text:?} was accepted"
        );
    }
}
