use std::time::{SystemTime, UNIX_EPOCH};

use jiff::Timestamp;

use crate::fields::push_formatted;

/// A clock before 1970 reads as 1970, one past year 9999 as `jiff`'s latest instant.
pub(crate) fn push_timestamp(line: &mut String, instant: SystemTime) {
    let timestamp = Timestamp::try_from(instant.max(UNIX_EPOCH)).unwrap_or(Timestamp::MAX);
    push_formatted(line, format_args!("{timestamp:.6}"));
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::push_timestamp;

    fn at(seconds: u64, nanos: u64) -> String {
        let mut line = String::new();
        let instant = UNIX_EPOCH + Duration::from_secs(seconds) + Duration::from_nanos(nanos);
        push_timestamp(&mut line, instant);
        line
    }

    #[test]
    fn instants_render_in_utc_with_microseconds() {
        assert_eq!(at(0, 0), "1970-01-01T00:00:00.000000Z");
        assert_eq!(at(951_782_400, 7_000), "2000-02-29T00:00:00.000007Z");
        assert_eq!(
            at(1_709_251_199, 999_999_999),
            "2024-02-29T23:59:59.999999Z"
        );
        assert_eq!(at(1_791_709_200, 0), "2026-10-11T09:00:00.000000Z");
        assert_eq!(at(4_102_444_800, 0), "2100-01-01T00:00:00.000000Z");
    }

    #[test]
    fn instants_outside_the_supported_range_are_clamped() {
        let mut before_1970 = String::new();
        push_timestamp(&mut before_1970, UNIX_EPOCH - Duration::from_secs(1));
        assert_eq!(before_1970, "1970-01-01T00:00:00.000000Z");

        assert_eq!(at(400_000_000_000, 0), "9999-12-30T22:00:00.999999Z");
    }
}
