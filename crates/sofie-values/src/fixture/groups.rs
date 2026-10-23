//! Each fixture is read from client JSON, so it is valid by the value's own check.

#![allow(
    clippy::unreadable_literal,
    reason = "samples are written as clients send them"
)]

use serde::de::DeserializeOwned;
use serde_json::json;

use super::{Fixture, Samples, Seed};

fn parsed<T: DeserializeOwned>(json: serde_json::Value) -> T {
    serde_json::from_value(json).expect("a fixture is valid JSON of its value")
}

#[cfg(feature = "percentage")]
mod percentage {
    use super::{Fixture, Samples, Seed, json, parsed};
    use crate::Percentage;

    impl Fixture for Percentage {
        fn fixture(seed: &mut Seed) -> Self {
            parsed(json!(seed.between(50, 2_500)))
        }
    }

    samples!(Percentage, accepted: ["1900", "0", "\"19.5\""], refused: ["\"19%\"", "1.5"]);
}

#[cfg(feature = "person")]
mod person {
    use super::{Fixture, Samples, Seed, json, parsed};
    use crate::PersonName;
    use crate::fixture::{FIRST_NAMES, LAST_NAMES};

    impl Fixture for PersonName {
        fn fixture(seed: &mut Seed) -> Self {
            let name = format!("{} {}", seed.pick(&FIRST_NAMES), seed.pick(&LAST_NAMES));
            parsed(json!(name))
        }
    }

    samples!(PersonName, accepted: ["\"Ada Lovelace\"", "\"Prince\""], refused: ["\"\"", "\"   \""]);
}

#[cfg(feature = "phone")]
mod phone {
    use super::{Fixture, Samples, Seed, json, parsed};
    use crate::PhoneNumber;

    /// In the range kept for drama, so it rings nobody.
    impl Fixture for PhoneNumber {
        fn fixture(seed: &mut Seed) -> Self {
            parsed(json!(format!("+49 30 2390{:04}", seed.between(0, 9_999))))
        }
    }

    samples!(PhoneNumber, accepted: ["\"+49 30 23901234\"", "\"+1 202 555 0143\""], refused: ["\"555\"", "\"+\"", "\"phone\""]);
}

#[cfg(feature = "geo")]
mod geo {
    use super::{Fixture, Samples, Seed, json, parsed};
    use crate::Coordinate;

    const CITIES: [(f64, f64); 4] = [
        (52.52, 13.405),
        (41.0082, 28.9784),
        (48.8566, 2.3522),
        (51.5074, -0.1278),
    ];

    impl Fixture for Coordinate {
        fn fixture(seed: &mut Seed) -> Self {
            let (latitude, longitude) = *seed.pick(&CITIES);
            parsed(json!({ "latitude": latitude, "longitude": longitude }))
        }
    }

    samples!(
        Coordinate,
        accepted: [r#"{"latitude":52.52,"longitude":13.405}"#, r#"{"latitude":"52.52","longitude":"-13.4"}"#],
        refused: [r#"{"latitude":91,"longitude":0}"#, r#"{"latitude":0,"longitude":181}"#, r#"{"latitude":"north","longitude":"0"}"#]
    );
}

#[cfg(feature = "measure")]
mod measure {
    use super::{Fixture, Samples, Seed, json, parsed};
    use crate::{Energy, Length, Measurement, Unit, Weight};

    impl<U: Unit> Fixture for Measurement<U> {
        fn fixture(seed: &mut Seed) -> Self {
            let unit = *seed.pick(U::ALL);
            let quantity = format!("{}.{}", seed.between(1, 250), seed.between(0, 9));
            parsed(json!({ "quantity": quantity, "unit": unit.name() }))
        }
    }

    samples!(Weight, accepted: [r#"{"quantity":"1.5","unit":"KILOGRAM"}"#], refused: [r#"{"quantity":"1.5","unit":"STONE"}"#, r#"{"quantity":"heavy","unit":"GRAM"}"#]);
    samples!(Length, accepted: [r#"{"quantity":"12","unit":"METRE"}"#], refused: [r#"{"quantity":"1","unit":"PARSEC"}"#]);
    samples!(Energy, accepted: [r#"{"quantity":"250","unit":"KILOCALORIE"}"#], refused: [r#"{"quantity":"1","unit":"ERG"}"#]);
}

#[cfg(feature = "time")]
mod time {
    use super::{Fixture, Samples, Seed, json, parsed};
    use sofie_core::Variants;

    use crate::{Date, LocalDateTime, TimeOfDay, TimeZone, Weekday};

    const ZONES: [&str; 4] = [
        "Europe/Berlin",
        "Europe/Istanbul",
        "America/New_York",
        "Asia/Tokyo",
    ];

    fn date_text(seed: &mut Seed) -> String {
        format!("2026-{:02}-{:02}", seed.between(1, 12), seed.between(1, 28))
    }

    impl Fixture for Date {
        fn fixture(seed: &mut Seed) -> Self {
            parsed(json!(date_text(seed)))
        }
    }

    impl Fixture for TimeZone {
        fn fixture(seed: &mut Seed) -> Self {
            parsed(json!(seed.pick(&ZONES)))
        }
    }

    impl Fixture for LocalDateTime {
        fn fixture(seed: &mut Seed) -> Self {
            let local = format!(
                "{}T{:02}:{:02}:00",
                date_text(seed),
                seed.between(9, 17),
                30 * seed.between(0, 1)
            );
            parsed(json!({ "local": local, "zone": seed.pick(&ZONES) }))
        }
    }

    impl Fixture for TimeOfDay {
        fn fixture(seed: &mut Seed) -> Self {
            let time = format!("{:02}:{:02}", seed.between(6, 22), 15 * seed.between(0, 3));
            parsed(json!(time))
        }
    }

    impl Fixture for Weekday {
        fn fixture(seed: &mut Seed) -> Self {
            let names: Vec<&str> = Weekday::VARIANTS.iter().map(|(name, _)| *name).collect();
            parsed(json!(seed.pick(&names)))
        }
    }

    samples!(TimeOfDay, accepted: ["\"09:30\"", "\"00:00\""], refused: ["\"24:00\"", "\"9:30\""]);
    samples!(Weekday, accepted: ["\"MONDAY\""], refused: ["\"monday\"", "\"FUNDAY\""]);
    samples!(Date, accepted: ["\"2026-10-11\""], refused: ["\"2026-02-30\"", "\"11.10.2026\""]);
    samples!(TimeZone, accepted: ["\"Europe/Istanbul\""], refused: ["\"Mars/Olympus\"", "\"\""]);
    samples!(
        LocalDateTime,
        accepted: [r#"{"local":"2026-10-11T09:30:00","zone":"Europe/Istanbul"}"#, r#"{"local":"2026-10-11T09:30","zone":"Europe/Istanbul"}"#],
        refused: [r#"{"local":"9:30","zone":"Europe/Istanbul"}"#, r#"{"local":"2026-10-11T09:30:00","zone":"Nowhere"}"#]
    );
}

#[cfg(feature = "web")]
mod web {
    use super::{Fixture, Samples, Seed, json, parsed};
    use crate::{Locale, Slug, Url};

    const LOCALES: [&str; 4] = ["en", "de-DE", "tr-TR", "fr"];
    const WORDS: [&str; 6] = ["spring", "sale", "linen", "shirt", "garden", "chair"];

    impl Fixture for Locale {
        fn fixture(seed: &mut Seed) -> Self {
            parsed(json!(seed.pick(&LOCALES)))
        }
    }

    impl Fixture for Slug {
        fn fixture(seed: &mut Seed) -> Self {
            parsed(json!(format!(
                "{}-{}",
                seed.pick(&WORDS),
                seed.pick(&WORDS)
            )))
        }
    }

    /// `example.com` serves nobody's real pages.
    impl Fixture for Url {
        fn fixture(seed: &mut Seed) -> Self {
            parsed(json!(format!("https://example.com/{}", seed.pick(&WORDS))))
        }
    }

    samples!(Locale, accepted: ["\"de-DE\"", "\"en\""], refused: ["\"german\"", "\"\""]);
    samples!(Slug, accepted: ["\"spring-sale\""], refused: ["\"Spring Sale\"", "\"\""]);
    samples!(Url, accepted: ["\"https://example.com/a?b=c\""], refused: ["\"example\"", "\"javascript:alert(1)\""]);
}
