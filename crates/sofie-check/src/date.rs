use std::time::{SystemTime, UNIX_EPOCH};

const SECONDS_PER_DAY: u64 = 86_400;

/// A UTC calendar day, counted from 1970-01-01.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date(i64);

impl Date {
    /// The one system time read outside `Clock`: a flag's age is measured on the day `sofie check` runs.
    #[must_use]
    pub fn today() -> Self {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Self(i64::try_from(since_epoch.as_secs() / SECONDS_PER_DAY).unwrap_or(i64::MAX))
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let shaped = text.len() == 10
            && text.bytes().enumerate().all(|(index, byte)| match index {
                4 | 7 => byte == b'-',
                _ => byte.is_ascii_digit(),
            });
        if !shaped {
            return None;
        }
        let year: i64 = text[0..4].parse().ok()?;
        let month: i64 = text[5..7].parse().ok()?;
        let day: i64 = text[8..10].parse().ok()?;
        let valid = (1..=12).contains(&month) && (1..=days_in_month(year, month)).contains(&day);
        valid.then(|| Self(days_from_epoch(year, month, day)))
    }

    #[must_use]
    pub fn days_since(self, earlier: Self) -> i64 {
        self.0 - earlier.0
    }
}

fn days_in_month(year: i64, month: i64) -> i64 {
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

// Howard Hinnant's `days_from_civil`: https://howardhinnant.github.io/date_algorithms.html
fn days_from_epoch(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::Date;

    #[test]
    fn counts_days_across_months_and_leap_years() {
        let day = |text| Date::parse(text).unwrap();
        assert_eq!(day("1970-01-01").days_since(day("1970-01-01")), 0);
        assert_eq!(day("2024-03-01").days_since(day("2024-02-28")), 2);
        assert_eq!(day("2026-10-14").days_since(day("2025-10-14")), 365);
        assert_eq!(day("2000-03-01").days_since(day("1970-01-01")), 11_017);
    }

    #[test]
    fn refuses_anything_but_a_real_day() {
        for refused in [
            "2026-02-29",
            "2026-13-01",
            "2026-00-10",
            "2026-9-01",
            "+026-09-01",
            "",
        ] {
            assert_eq!(Date::parse(refused), None, "{refused}");
        }
    }
}
