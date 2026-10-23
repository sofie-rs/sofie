//! The same seed gives the same values, so a failing test fails again.

macro_rules! samples {
    ($value:ty, accepted: [$($accepted:literal),*], refused: [$($refused:literal),*]) => {
        impl Samples for $value {
            const ACCEPTED: &'static [&'static str] = &[$($accepted),*];
            const REFUSED: &'static [&'static str] = &[$($refused),*];
        }
    };
}

mod calls;
#[cfg(any(
    feature = "geo",
    feature = "measure",
    feature = "percentage",
    feature = "person",
    feature = "phone",
    feature = "time",
    feature = "web"
))]
mod groups;

use serde::Serialize;
use serde_json::Value as Json;
use sofie_core::{Constraint, Instant, Uuid};

use crate::{Currency, Email, Money, Secret};

pub use calls::Calls;

/// `SplitMix64`: good enough for picking names, not for anything secret.
#[derive(Clone, Debug, Default)]
pub struct Seed {
    state: u64,
}

impl Seed {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn draw(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut mixed = self.state;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        mixed ^ (mixed >> 31)
    }

    pub fn between(&mut self, low: i64, high: i64) -> i64 {
        let span = high.abs_diff(low).saturating_add(1);
        low.saturating_add_unsigned(self.draw() % span)
    }

    pub fn pick<'a, T>(&mut self, choices: &'a [T]) -> &'a T {
        let index = self.draw() % choices.len() as u64;
        &choices[usize::try_from(index).expect("an index of a slice fits usize")]
    }
}

pub trait Fixture: Sized {
    fn fixture(seed: &mut Seed) -> Self;
}

pub trait Samples {
    const ACCEPTED: &'static [&'static str];
    const REFUSED: &'static [&'static str];
}

pub trait InputFixture {
    fn fixture_json(seed: &mut Seed) -> Json;
}

pub(crate) const FIRST_NAMES: [&str; 12] = [
    "Ada", "Grace", "Alan", "Edsger", "Barbara", "Donald", "Frances", "Ken", "Margaret", "Niklaus",
    "Radia", "Tim",
];
pub(crate) const LAST_NAMES: [&str; 12] = [
    "Lovelace",
    "Hopper",
    "Turing",
    "Dijkstra",
    "Liskov",
    "Knuth",
    "Allen",
    "Thompson",
    "Hamilton",
    "Wirth",
    "Perlman",
    "Berners-Lee",
];
const WORDS: [&str; 16] = [
    "amber", "birch", "cobalt", "delta", "ember", "fjord", "granite", "harbor", "indigo",
    "juniper", "kestrel", "linen", "meadow", "nectar", "orchard", "pepper",
];
const CURRENCIES: [&str; 4] = ["EUR", "USD", "GBP", "CHF"];
/// 2026-01-01T00:00:00Z.
const FIRST_SECOND: i64 = 1_767_225_600;
const SECONDS_PER_YEAR: i64 = 365 * 86_400;

/// `id` stands in for an `Id<…>` field, which names a record by its uuid.
#[must_use]
pub fn field_json<T: Fixture + Serialize>(seed: &mut Seed, constraints: &[Constraint]) -> Json {
    let json = serde_json::to_value(T::fixture(seed)).expect("a fixture serializes to JSON");
    fitted(json, constraints)
}

pub fn variant<E: sofie_core::Variants>(seed: &mut Seed) -> E {
    if E::TRANSITIONS.is_empty() {
        return seed.pick(E::VARIANTS).1;
    }
    E::VARIANTS[0].1
}

fn fitted(json: Json, constraints: &[Constraint]) -> Json {
    constraints
        .iter()
        .fold(json, |json, constraint| match (json, *constraint) {
            (Json::String(text), Constraint::MaxLength(max)) => {
                Json::String(text.chars().take(count(max)).collect())
            }
            (Json::String(mut text), Constraint::MinLength(min)) => {
                while text.chars().count() < count(min) {
                    text.push('x');
                }
                Json::String(text)
            }
            (Json::Number(number), Constraint::Max(max)) => {
                Json::from(number.as_i64().map_or(max, |number| number.min(max)))
            }
            (Json::Number(number), Constraint::Min(min)) => {
                Json::from(number.as_i64().map_or(min, |number| number.max(min)))
            }
            (json, _) => json,
        })
}

fn count(bound: i64) -> usize {
    usize::try_from(bound.max(0)).unwrap_or(usize::MAX)
}

fn words(seed: &mut Seed) -> String {
    format!("{} {}", seed.pick(&WORDS), seed.pick(&WORDS))
}

macro_rules! numbers {
    ($($number:ty: $low:literal..=$high:literal),*) => {$(
        impl Fixture for $number {
            fn fixture(seed: &mut Seed) -> Self {
                <$number>::try_from(seed.between($low, $high)).expect("the range fits the type")
            }
        }
    )*};
}

numbers!(i16: 1..=999, i32: 1..=9_999, i64: 1..=99_999, u16: 1..=999, u32: 1..=9_999);

impl Fixture for bool {
    fn fixture(seed: &mut Seed) -> Self {
        seed.draw().is_multiple_of(2)
    }
}

impl Fixture for String {
    fn fixture(seed: &mut Seed) -> Self {
        words(seed)
    }
}

impl<T: Fixture> Fixture for Option<T> {
    fn fixture(seed: &mut Seed) -> Self {
        Some(T::fixture(seed))
    }
}

impl<T: Fixture> Fixture for Vec<T> {
    fn fixture(seed: &mut Seed) -> Self {
        (0..seed.between(1, 3)).map(|_| T::fixture(seed)).collect()
    }
}

impl<T: Fixture> Fixture for Secret<T> {
    fn fixture(seed: &mut Seed) -> Self {
        Self::new(T::fixture(seed))
    }
}

impl Fixture for Uuid {
    fn fixture(seed: &mut Seed) -> Self {
        let bits = u128::from(seed.draw()) << 64 | u128::from(seed.draw());
        let version = bits & !(0xF << 76) | (0x4 << 76);
        Self::from_u128(version & !(0x3 << 62) | (0x2 << 62))
    }
}

/// A whole second, as a database stores it exactly.
impl Fixture for Instant {
    fn fixture(seed: &mut Seed) -> Self {
        let second = FIRST_SECOND + seed.between(0, SECONDS_PER_YEAR - 1);
        sofie_core::Timestamp::from_second(second)
            .expect("a second of 2026 is a timestamp")
            .into()
    }
}

samples!(Instant, accepted: ["\"2026-10-11T09:30:00Z\"", "\"2026-10-11T12:30:00+03:00\""], refused: ["\"yesterday\"", "\"2026-13-01T00:00:00Z\"", "42"]);

impl Fixture for Currency {
    fn fixture(seed: &mut Seed) -> Self {
        Currency::parse(seed.pick(&CURRENCIES)).expect("a listed currency parses")
    }
}

samples!(Currency, accepted: ["\"EUR\"", "\"JPY\""], refused: ["\"eur\"", "\"EURO\"", "\"\""]);

impl Fixture for Money {
    fn fixture(seed: &mut Seed) -> Self {
        let currency = Currency::fixture(seed);
        Money::new(seed.between(100, 99_999), currency)
    }
}

samples!(
    Money,
    accepted: [r#"{"minor":14990,"currency":"EUR"}"#, r#"{"amount":"12.50","currency":"USD"}"#],
    refused: [r#"{"minor":100,"currency":"euro"}"#, r#"{"amount":"1.234","currency":"EUR"}"#, r#""12.50 EUR""#]
);

/// `example.com` never reaches anyone.
impl Fixture for Email {
    fn fixture(seed: &mut Seed) -> Self {
        let first = seed.pick(&FIRST_NAMES).to_lowercase();
        let last = seed.pick(&LAST_NAMES).to_lowercase();
        let number = seed.between(1, 999);
        Email::parse(&format!("{first}.{last}{number}@example.com"))
            .expect("a fixture address parses")
    }
}

samples!(Email, accepted: ["\"ada@example.com\"", "\"Grace.Hopper@Example.COM\""], refused: ["\"ada\"", "\"ada@\"", "\"@example.com\"", "\"\""]);

#[cfg(test)]
mod tests {
    use super::*;

    fn json<T: Fixture + Serialize>(seed: u64) -> Json {
        serde_json::to_value(T::fixture(&mut Seed::new(seed))).unwrap()
    }

    #[test]
    fn the_same_seed_makes_the_same_fixtures_and_another_seed_others() {
        assert_eq!(json::<Email>(7), json::<Email>(7));
        assert_ne!(json::<Email>(7), json::<Email>(8));
        assert_eq!(json::<Money>(7), json::<Money>(7));
    }

    #[test]
    fn fixtures_look_like_what_people_enter() {
        let email = json::<Email>(1);
        let money = Money::fixture(&mut Seed::new(1));
        let instant = Instant::fixture(&mut Seed::new(1));

        assert!(email.as_str().unwrap().ends_with("@example.com"), "{email}");
        assert!((100..=99_999).contains(&money.minor()), "{money:?}");
        assert!(
            instant.timestamp().as_second() >= FIRST_SECOND,
            "{instant:?}"
        );
        assert_eq!(instant.timestamp().subsec_nanosecond(), 0);
    }

    #[test]
    fn a_uuid_fixture_is_a_version_4_uuid() {
        let uuid = Uuid::fixture(&mut Seed::new(3)).to_string();

        assert_eq!(uuid.as_bytes()[14], b'4', "{uuid}");
        assert!(
            matches!(uuid.as_bytes()[19], b'8' | b'9' | b'a' | b'b'),
            "{uuid}"
        );
    }

    #[test]
    fn a_fitted_fixture_keeps_its_field_s_constraints() {
        let long = Json::from("cobalt harbor");

        assert_eq!(fitted(long.clone(), &[Constraint::MaxLength(6)]), "cobalt");
        assert_eq!(
            fitted(Json::from("ab"), &[Constraint::MinLength(4)]),
            "abxx"
        );
        assert_eq!(fitted(Json::from(500), &[Constraint::Max(99)]), 99);
        assert_eq!(fitted(Json::from(5), &[Constraint::Min(10)]), 10);
        assert_eq!(fitted(long.clone(), &[Constraint::Required]), long);
    }

    fn check<T: Fixture + Samples + Serialize + serde::de::DeserializeOwned>() {
        let fixture = serde_json::to_value(T::fixture(&mut Seed::new(5))).unwrap();
        assert!(
            serde_json::from_value::<T>(fixture.clone()).is_ok(),
            "{fixture}"
        );
        for accepted in T::ACCEPTED {
            assert!(serde_json::from_str::<T>(accepted).is_ok(), "{accepted}");
        }
        for refused in T::REFUSED {
            assert!(serde_json::from_str::<T>(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn each_value_s_fixture_and_samples_parse_as_they_say() {
        check::<Instant>();
        check::<Currency>();
        check::<Money>();
        check::<Email>();
        #[cfg(feature = "percentage")]
        check::<crate::Percentage>();
        #[cfg(feature = "person")]
        check::<crate::PersonName>();
        #[cfg(feature = "phone")]
        check::<crate::PhoneNumber>();
        #[cfg(feature = "geo")]
        check::<crate::Coordinate>();
        #[cfg(feature = "measure")]
        {
            check::<crate::Weight>();
            check::<crate::Length>();
            check::<crate::Energy>();
        }
        #[cfg(feature = "time")]
        {
            check::<crate::Date>();
            check::<crate::TimeZone>();
            check::<crate::LocalDateTime>();
            check::<crate::TimeOfDay>();
            check::<crate::Weekday>();
        }
        #[cfg(feature = "web")]
        {
            check::<crate::Locale>();
            check::<crate::Slug>();
            check::<crate::Url>();
        }
    }
}
