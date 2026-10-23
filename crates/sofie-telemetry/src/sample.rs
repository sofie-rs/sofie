use crate::InitError;
use crate::init::variable_or;

pub(crate) const LOG_SAMPLE_VARIABLE: &str = "SOFIE_LOG_SAMPLE";
const UNSET_RATIO: &str = "1";
/// 2^63: the ratio is compared with 63 random bits of the trace id.
const RANDOM_RANGE: f64 = 9_223_372_036_854_775_808.0;

/// Decided by trace id as OpenTelemetry's ratio sampler does, so a trace is kept or dropped
/// together in every process it crosses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Sample {
    bound: Option<u64>,
}

impl Sample {
    pub(crate) const EVERY: Self = Self { bound: None };

    pub(crate) fn from_environment(variable: &str) -> Result<Self, InitError> {
        let text = variable_or(variable, UNSET_RATIO)?;
        Self::parse(&text).ok_or_else(|| {
            InitError::new(format!(
                "{variable} `{text}` is not a ratio from 0 to 1, such as 0.1"
            ))
        })
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        let ratio: f64 = text.trim().parse().ok()?;
        if !(0.0..=1.0).contains(&ratio) {
            return None;
        }
        if ratio >= 1.0 {
            return Some(Self::EVERY);
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the product lies in 0..2^63, which a u64 holds"
        )]
        let bound = (ratio * RANDOM_RANGE) as u64;
        Some(Self { bound: Some(bound) })
    }

    pub(crate) fn keeps(self, trace_id: u128) -> bool {
        let Some(bound) = self.bound else {
            return true;
        };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the low 8 bytes carry the trace id's randomness"
        )]
        let random = (trace_id as u64) >> 1;
        random < bound
    }
}

#[cfg(test)]
mod tests {
    use super::Sample;

    #[test]
    fn unset_or_one_keeps_every_trace_and_zero_none() {
        let every = Sample::parse("1").unwrap();
        let none = Sample::parse("0").unwrap();

        for trace_id in [0, 1, u128::MAX, u128::from(u64::MAX) << 64] {
            assert!(every.keeps(trace_id));
            assert!(!none.keeps(trace_id));
        }
    }

    #[test]
    fn a_ratio_keeps_about_that_share_of_traces() {
        let tenth = Sample::parse(" 0.1 ").unwrap();
        let step = u64::MAX / 1000;
        let kept = (0..1000u64)
            .filter(|index| tenth.keeps(u128::from(index * step)))
            .count();

        assert!((95..=105).contains(&kept), "{kept}");
    }

    #[test]
    fn anything_but_a_ratio_is_refused() {
        for text in ["", "half", "-0.1", "1.5", "NaN", "inf"] {
            assert_eq!(Sample::parse(text), None, "{text}");
        }
    }
}
