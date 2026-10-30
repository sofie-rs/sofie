use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Error;
use crate::encoding::hex;

/// RFC 9562; ordered by bytes so version 7 ids sort in creation order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Uuid([u8; 16]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseUuidError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RandomnessUnavailable;

const COUNTER_BITS: u32 = 12;
const COUNTER_MASK: u64 = (1 << COUNTER_BITS) - 1;
/// Starts mid-range so at least 2048 ids fit before the timestamp has to advance.
const SEED_MASK: u64 = COUNTER_MASK >> 1;
const TIMESTAMP_MAX: u64 = (1 << 48) - 1;
const HYPHEN_POSITIONS: [usize; 4] = [8, 13, 18, 23];

/// `timestamp << 12 | counter`; process-wide so ids from any thread sort in creation order.
static LAST_V7: AtomicU64 = AtomicU64::new(0);

impl Uuid {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn from_u128(value: u128) -> Self {
        Self(value.to_be_bytes())
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    pub fn push_to(self, out: &mut String) {
        out.push_str(self.hex().as_str());
    }

    pub fn now_v7() -> Result<Self, RandomnessUnavailable> {
        let mut random = [0; 10];
        crate::random::fill(&mut random)?;
        let [seed_high, seed_low, rest @ ..] = random;
        let seed = u64::from(u16::from_be_bytes([seed_high, seed_low])) & SEED_MASK;
        let state = advance_v7_state(seed);
        let timestamp_and_counter = (state >> COUNTER_BITS) << 16 | (state & COUNTER_MASK);
        let mut bytes = [0; 16];
        bytes[..8].copy_from_slice(&timestamp_and_counter.to_be_bytes());
        bytes[8..].copy_from_slice(&rest);
        Ok(Self(bytes).with_version(7))
    }

    pub fn new_v4() -> Result<Self, RandomnessUnavailable> {
        let mut bytes = [0; 16];
        crate::random::fill(&mut bytes)?;
        Ok(Self(bytes).with_version(4))
    }

    const fn with_version(mut self, version: u8) -> Self {
        self.0[6] = (self.0[6] & 0x0f) | (version << 4);
        self.0[8] = (self.0[8] & 0x3f) | 0x80;
        self
    }

    fn hex(self) -> Hex {
        let mut text = [b'-'; 36];
        let mut position = 0;
        for (index, byte) in self.0.into_iter().enumerate() {
            if matches!(index, 4 | 6 | 8 | 10) {
                position += 1;
            }
            text[position..position + 2].copy_from_slice(&hex::digits(byte));
            position += 2;
        }
        Hex(text)
    }
}

struct Hex([u8; 36]);

impl Hex {
    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap_or_default()
    }
}

/// Lock-free: a thread that loses the race retries from the winner's state.
fn advance_v7_state(seed: u64) -> u64 {
    let mut previous = LAST_V7.load(Ordering::Relaxed);
    loop {
        let next = next_v7_state(previous, unix_millis(), seed);
        match LAST_V7.compare_exchange_weak(previous, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(current) => previous = current,
        }
    }
}

fn next_v7_state(previous: u64, now_millis: u64, seed: u64) -> u64 {
    if now_millis > previous >> COUNTER_BITS {
        return now_millis << COUNTER_BITS | seed;
    }
    previous + 1
}

/// A clock before 1970 reads as 0; ids still count up from the last one, so order holds.
fn unix_millis() -> u64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(elapsed.as_millis())
        .unwrap_or(u64::MAX)
        .min(TIMESTAMP_MAX)
}

impl FromStr for Uuid {
    type Err = ParseUuidError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let hyphenated = match text.len() {
            36 => true,
            32 => false,
            _ => return Err(ParseUuidError),
        };
        let mut value = 0_u128;
        for (index, byte) in text.bytes().enumerate() {
            if hyphenated && HYPHEN_POSITIONS.contains(&index) {
                if byte != b'-' {
                    return Err(ParseUuidError);
                }
                continue;
            }
            let digit = hex::digit_value(byte).ok_or(ParseUuidError)?;
            value = value << 4 | u128::from(digit);
        }
        Ok(Self::from_u128(value))
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.hex().as_str())
    }
}

impl fmt::Debug for Uuid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

/// Written whole, so a serializer that builds a string allocates it once.
impl Serialize for Uuid {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.hex().as_str())
    }
}

impl<'de> Deserialize<'de> for Uuid {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        crate::deserialize_parsed(deserializer, "a UUID", str::parse)
    }
}

impl fmt::Display for ParseUuidError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("not a UUID: expected 32 hex digits, optionally hyphenated 8-4-4-4-12")
    }
}

impl std::error::Error for ParseUuidError {}

impl fmt::Display for RandomnessUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the operating system's random number generator failed")
    }
}

impl std::error::Error for RandomnessUnavailable {}

impl From<RandomnessUnavailable> for Error {
    fn from(error: RandomnessUnavailable) -> Self {
        Self::internal("generating a UUID", error)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::thread;

    use super::*;

    const RFC_V7: &str = "017f22e2-79b0-7cc3-98c4-dc0c0c07398f";
    const RFC_V4: &str = "919108f7-52d1-4320-9bac-f847db4148a8";

    fn version(id: Uuid) -> u8 {
        id.as_bytes()[6] >> 4
    }

    fn variant(id: Uuid) -> u8 {
        id.as_bytes()[8] >> 6
    }

    fn assert_strictly_increasing(ids: &[Uuid]) {
        for pair in ids.windows(2) {
            assert!(pair[0] < pair[1], "{} then {}", pair[0], pair[1]);
        }
    }

    #[test]
    fn the_rfc_9562_examples_parse_and_format_back() {
        let v7: Uuid = RFC_V7.parse().unwrap();
        let v4: Uuid = RFC_V4.parse().unwrap();

        assert_eq!(
            v7,
            Uuid::from_u128(0x017f_22e2_79b0_7cc3_98c4_dc0c_0c07_398f)
        );
        assert_eq!(v7.to_string(), RFC_V7);
        assert_eq!(v4.to_string(), RFC_V4);
        assert_eq!((version(v7), variant(v7)), (7, 0b10));
        assert_eq!((version(v4), variant(v4)), (4, 0b10));
    }

    #[test]
    fn upper_case_and_simple_forms_parse_to_the_same_id() {
        let expected: Uuid = RFC_V7.parse().unwrap();

        assert_eq!(RFC_V7.to_uppercase().parse::<Uuid>().unwrap(), expected);
        assert_eq!(
            "017f22e279b07cc398c4dc0c0c07398f".parse::<Uuid>().unwrap(),
            expected
        );
        assert_eq!(format!("{expected:?}"), RFC_V7);
    }

    #[test]
    fn every_nibble_is_written_in_place_between_the_hyphens() {
        let id = Uuid::from_bytes([
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ]);

        assert_eq!(id.to_string(), "01234567-89ab-cdef-fedc-ba9876543210");
    }

    #[test]
    fn bytes_round_trip_in_network_order() {
        let id: Uuid = RFC_V7.parse().unwrap();

        assert_eq!(id.as_bytes()[..4], [0x01, 0x7f, 0x22, 0xe2]);
        assert_eq!(Uuid::from_bytes(*id.as_bytes()), id);
    }

    #[test]
    fn anything_but_the_two_forms_is_refused() {
        let refused = [
            "",
            "017f22e2-79b0-7cc3-98c4-dc0c0c07398",
            "017f22e2-79b0-7cc3-98c4-dc0c0c07398f0",
            "017f22e279b07cc398c4dc0c0c07398",
            "017f22e279b07cc398c4dc0c0c07398f0",
            "017f22e2-79b0-7cc3-98c4-dc0c0c07398g",
            "017f22e279b0-7cc3-98c4-dc0c0c07398f-",
            "017f22e2_79b0_7cc3_98c4_dc0c0c07398f",
            "{017f22e2-79b0-7cc3-98c4-dc0c0c0739}",
            "urn:uuid:017f22e2-79b0-7cc3-98c4-dc0c0c07398f",
            " 17f22e279b07cc398c4dc0c0c07398f",
            "+17f22e279b07cc398c4dc0c0c07398f",
            "017f22e2-79b0-7cc3-98c4-dc0c0c0739é",
        ];

        for text in refused {
            assert_eq!(text.parse::<Uuid>(), Err(ParseUuidError), "{text:?}");
        }
    }

    #[test]
    fn generated_ids_carry_their_version_and_variant() {
        for _ in 0..1000 {
            let v7 = Uuid::now_v7().unwrap();
            let v4 = Uuid::new_v4().unwrap();

            assert_eq!((version(v7), variant(v7)), (7, 0b10));
            assert_eq!((version(v4), variant(v4)), (4, 0b10));
        }
    }

    #[test]
    fn a_v7_id_starts_with_the_current_unix_milliseconds() {
        let before = unix_millis();
        let id = Uuid::now_v7().unwrap();
        let after = unix_millis();

        let mut timestamp = [0; 8];
        timestamp[2..].copy_from_slice(&id.as_bytes()[..6]);
        let millis = u64::from_be_bytes(timestamp);
        // Parallel tests can overflow the counter, which moves the timestamp ahead of the clock.
        assert!(
            before <= millis && millis <= after + 1_000,
            "{before} {millis} {after}"
        );
    }

    #[test]
    fn ids_made_in_a_tight_loop_sort_in_creation_order() {
        let ids: Vec<Uuid> = (0..100_000).map(|_| Uuid::now_v7().unwrap()).collect();

        assert_strictly_increasing(&ids);
    }

    #[test]
    fn ids_made_on_many_threads_are_unique_and_ordered_per_thread() {
        let per_thread: Vec<Vec<Uuid>> = thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| (0..20_000).map(|_| Uuid::now_v7().unwrap()).collect()))
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        });

        for ids in &per_thread {
            assert_strictly_increasing(ids);
        }
        let unique: HashSet<Uuid> = per_thread.iter().flatten().copied().collect();
        assert_eq!(unique.len(), 8 * 20_000);
    }

    #[test]
    fn ids_made_one_after_another_across_threads_sort_in_that_order() {
        let made = Mutex::new(Vec::new());
        thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..5_000 {
                        let mut made = made.lock().unwrap();
                        made.push(Uuid::now_v7().unwrap());
                    }
                });
            }
        });

        assert_strictly_increasing(&made.into_inner().unwrap());
    }

    #[test]
    fn a_new_millisecond_restarts_the_counter_at_the_seed() {
        let previous = 41 << COUNTER_BITS | 0x384;

        assert_eq!(next_v7_state(previous, 42, 7), 42 << COUNTER_BITS | 7);
    }

    #[test]
    fn the_same_or_an_earlier_millisecond_counts_up() {
        let previous = 42 << COUNTER_BITS | 0x384;

        assert_eq!(next_v7_state(previous, 42, 7), previous + 1);
        assert_eq!(next_v7_state(previous, 30, 7), previous + 1);
    }

    #[test]
    fn counter_overflow_advances_the_timestamp_field() {
        let previous = 42 << COUNTER_BITS | COUNTER_MASK;

        assert_eq!(next_v7_state(previous, 42, 7), 43 << COUNTER_BITS);
    }

    #[test]
    fn serde_uses_the_hyphenated_string() {
        let id: Uuid = RFC_V7.parse().unwrap();

        let json = serde_json::to_string(&id).unwrap();
        let owned: Uuid = serde_json::from_value(serde_json::Value::String(RFC_V7.into())).unwrap();

        assert_eq!(json, format!("\"{RFC_V7}\""));
        assert_eq!(serde_json::from_str::<Uuid>(&json).unwrap(), id);
        assert_eq!(owned, id);
        assert!(serde_json::from_str::<Uuid>("\"not a uuid\"").is_err());
        assert!(serde_json::from_str::<Uuid>("42").is_err());
    }
}
