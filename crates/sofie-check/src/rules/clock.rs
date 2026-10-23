use std::path::Path;

use super::{Finding, Rule};

const TIME_TYPES: [&str; 9] = [
    "SystemTime",
    "Instant",
    "Timestamp",
    "Zoned",
    "Utc",
    "Local",
    "OffsetDateTime",
    "PrimitiveDateTime",
    "UtcDateTime",
];
const READING_FUNCTIONS: [&str; 4] = ["now", "now_utc", "now_local", "today"];

pub(super) fn reads_system_time(segments: &[String]) -> bool {
    segments.windows(2).any(|pair| {
        TIME_TYPES.contains(&pair[0].as_str()) && READING_FUNCTIONS.contains(&pair[1].as_str())
    })
}

pub(super) fn finding(file: &Path, line: usize, written: &str) -> Finding {
    Finding {
        rule: Rule::SystemTime,
        file: file.to_path_buf(),
        line,
        message: format!(
            "`{written}` reads the system time, so tests and `sofie dev --at` cannot move it and \
             schedules cannot be replayed"
        ),
        fix: "ask for `Clock` and call `clock.now()`".to_owned(),
    }
}
