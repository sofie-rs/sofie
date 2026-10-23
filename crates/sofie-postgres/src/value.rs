use std::error::Error as StdError;

use bytes::{BufMut, BytesMut};
use jiff::civil::{Date, DateTime, date};
use jiff::tz::Offset;
use jiff::{Span, Timestamp};
use sofie_core::{SqlValue, Uuid};
use tokio_postgres::types::{FromSql, IsNull, ToSql, Type, WrongType};

pub(crate) type WireResult<T> = Result<T, Box<dyn StdError + Sync + Send>>;

const POSTGRES_EPOCH_MICROSECONDS: i64 = 946_684_800_000_000;
const POSTGRES_EPOCH_DATE: Date = date(2000, 1, 1);
const JSONB_VERSION: u8 = 1;

/// A parameter, encoded by the type the server inferred for its placeholder.
#[derive(Debug)]
pub(crate) struct Encoded<'a>(pub(crate) &'a SqlValue);

impl ToSql for Encoded<'_> {
    fn to_sql(&self, ty: &Type, out: &mut BytesMut) -> WireResult<IsNull> {
        match self.0 {
            SqlValue::Null => Ok(IsNull::Yes),
            SqlValue::Bool(value) => value.to_sql_checked(ty, out),
            SqlValue::Int(value) => encode_int(*value, ty, out),
            SqlValue::Float(value) => encode_float(*value, ty, out),
            SqlValue::Text(value) => value.as_str().to_sql_checked(ty, out),
            SqlValue::Bytes(value) => value.as_slice().to_sql_checked(ty, out),
            SqlValue::Uuid(value) => encode_uuid(*value, ty, out),
            SqlValue::Timestamp(value) => encode_timestamp(*value, ty, out),
            SqlValue::Date(value) => encode_date(*value, ty, out),
            SqlValue::DateTime(value) => encode_date_time(*value, ty, out),
            SqlValue::Json(value) => encode_json(value, ty, out),
            SqlValue::Array(values) => values
                .iter()
                .map(Encoded)
                .collect::<Vec<_>>()
                .to_sql_checked(ty, out),
        }
    }

    // Which types fit depends on the value, not on this wrapper, so `to_sql`
    // checks the type itself.
    fn accepts(_: &Type) -> bool {
        true
    }

    fn to_sql_checked(&self, ty: &Type, out: &mut BytesMut) -> WireResult<IsNull> {
        self.to_sql(ty, out)
    }
}

fn encode_int(value: i64, ty: &Type, out: &mut BytesMut) -> WireResult<IsNull> {
    match *ty {
        Type::INT2 => i16::try_from(value)?.to_sql_checked(ty, out),
        Type::INT4 => i32::try_from(value)?.to_sql_checked(ty, out),
        _ => value.to_sql_checked(ty, out),
    }
}

fn encode_float(value: f64, ty: &Type, out: &mut BytesMut) -> WireResult<IsNull> {
    if *ty != Type::FLOAT4 {
        return value.to_sql_checked(ty, out);
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a real column stores single precision; narrowing is what the column asks for"
    )]
    let narrowed = value as f32;
    narrowed.to_sql_checked(ty, out)
}

fn encode_uuid(value: Uuid, ty: &Type, out: &mut BytesMut) -> WireResult<IsNull> {
    expect_type::<Uuid>(ty, &Type::UUID)?;
    out.put_slice(value.as_bytes());
    Ok(IsNull::No)
}

fn encode_timestamp(value: Timestamp, ty: &Type, out: &mut BytesMut) -> WireResult<IsNull> {
    expect_type::<Timestamp>(ty, &Type::TIMESTAMPTZ)?;
    out.put_i64(value.as_microsecond() - POSTGRES_EPOCH_MICROSECONDS);
    Ok(IsNull::No)
}

/// `timestamp` counts microseconds from the same epoch as `timestamptz`, read as wall-clock time.
fn encode_date_time(value: DateTime, ty: &Type, out: &mut BytesMut) -> WireResult<IsNull> {
    expect_type::<DateTime>(ty, &Type::TIMESTAMP)?;
    let microseconds = Offset::UTC.to_timestamp(value)?.as_microsecond();
    out.put_i64(microseconds - POSTGRES_EPOCH_MICROSECONDS);
    Ok(IsNull::No)
}

fn encode_date(value: Date, ty: &Type, out: &mut BytesMut) -> WireResult<IsNull> {
    expect_type::<Date>(ty, &Type::DATE)?;
    out.put_i32(POSTGRES_EPOCH_DATE.until(value)?.get_days());
    Ok(IsNull::No)
}

/// `jsonb` is a version byte before the text; `json` is the text alone.
fn encode_json(value: &serde_json::Value, ty: &Type, out: &mut BytesMut) -> WireResult<IsNull> {
    match *ty {
        Type::JSONB => out.put_u8(JSONB_VERSION),
        Type::JSON => {}
        _ => return Err(Box::new(WrongType::new::<serde_json::Value>(ty.clone()))),
    }
    serde_json::to_writer(out.writer(), value)?;
    Ok(IsNull::No)
}

fn expect_type<T>(actual: &Type, expected: &Type) -> Result<(), WrongType> {
    if actual == expected {
        return Ok(());
    }
    Err(WrongType::new::<T>(actual.clone()))
}

pub(crate) fn decode_timestamp(raw: &[u8]) -> WireResult<Timestamp> {
    let microseconds = i64::from_sql(&Type::INT8, raw)?
        .checked_add(POSTGRES_EPOCH_MICROSECONDS)
        .ok_or("timestamp out of range")?;
    Ok(Timestamp::from_microsecond(microseconds)?)
}

pub(crate) fn decode_date_time(raw: &[u8]) -> WireResult<DateTime> {
    Ok(Offset::UTC.to_datetime(decode_timestamp(raw)?))
}

pub(crate) fn decode_date(raw: &[u8]) -> WireResult<Date> {
    let days = i32::from_sql(&Type::INT4, raw)?;
    Ok(POSTGRES_EPOCH_DATE.checked_add(Span::new().try_days(days)?)?)
}

pub(crate) fn decode_jsonb(raw: &[u8]) -> WireResult<serde_json::Value> {
    match raw.split_first() {
        Some((&JSONB_VERSION, text)) => Ok(serde_json::from_slice(text)?),
        _ => Err("unsupported jsonb version".into()),
    }
}
