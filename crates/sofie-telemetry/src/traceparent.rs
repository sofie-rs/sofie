use std::fmt;

const VERSION: &str = "00";
const INVALID_VERSION: &str = "ff";
const SAMPLED: u8 = 0x01;
const TRACE_ID_DIGITS: usize = 32;
const SPAN_ID_DIGITS: usize = 16;
const FLAGS_DIGITS: usize = 2;

/// A W3C trace context: <https://www.w3.org/TR/trace-context/>.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceParent {
    pub(crate) trace_id: u128,
    pub(crate) span_id: u64,
    pub(crate) flags: u8,
}

impl TraceParent {
    /// Refuses headers whose ids are all zeros. A later version may append fields, which
    /// are ignored as W3C asks; flags this version does not know are dropped.
    #[must_use]
    pub fn parse(header: &str) -> Option<Self> {
        let mut fields = header.split('-');
        let [Some(version), Some(trace_id), Some(span_id), Some(flags)] =
            std::array::from_fn(|_| fields.next())
        else {
            return None;
        };
        let well_formed = match lower_hex(version, VERSION.len())? {
            VERSION => fields.next().is_none(),
            INVALID_VERSION => false,
            _ => true,
        };
        if !well_formed {
            return None;
        }
        let parsed = Self {
            trace_id: u128::from_str_radix(lower_hex(trace_id, TRACE_ID_DIGITS)?, 16).ok()?,
            span_id: u64::from_str_radix(lower_hex(span_id, SPAN_ID_DIGITS)?, 16).ok()?,
            flags: u8::from_str_radix(lower_hex(flags, FLAGS_DIGITS)?, 16).ok()? & SAMPLED,
        };
        (parsed.trace_id != 0 && parsed.span_id != 0).then_some(parsed)
    }

    #[must_use]
    pub fn trace_id(&self) -> String {
        format!("{:032x}", self.trace_id)
    }

    pub(crate) fn root() -> Self {
        Self {
            trace_id: u128::from_be_bytes(nonzero_random()),
            span_id: new_span_id(),
            flags: SAMPLED,
        }
    }

    #[cfg(feature = "otlp")]
    pub(crate) fn unsampled(self) -> Self {
        Self { flags: 0, ..self }
    }

    pub(crate) fn child(self) -> Self {
        Self {
            span_id: new_span_id(),
            ..self
        }
    }
}

impl fmt::Display for TraceParent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{VERSION}-{:032x}-{:016x}-{:02x}",
            self.trace_id, self.span_id, self.flags
        )
    }
}

/// Upper case is refused, as the W3C format requires.
fn lower_hex(text: &str, digits: usize) -> Option<&str> {
    let well_formed = text.len() == digits
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    well_formed.then_some(text)
}

fn new_span_id() -> u64 {
    u64::from_be_bytes(nonzero_random())
}

/// The subscriber cannot return an error, and a system without randomness cannot run the
/// framework, so it panics. W3C forbids all-zero ids, hence the retry.
fn nonzero_random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    while bytes.iter().all(|&byte| byte == 0) {
        sofie_core::random::fill(&mut bytes)
            .expect("the operating system's random source is unavailable");
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::TraceParent;

    const VALID: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn a_valid_header_round_trips() {
        let parsed = TraceParent::parse(VALID).unwrap();

        assert_eq!(parsed.to_string(), VALID);
        assert_eq!(parsed.trace_id(), "4bf92f3577b34da6a3ce929d0e0e4736");
    }

    #[test]
    fn malformed_headers_are_refused() {
        for header in [
            "",
            "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
            "00-4bf92f3577b34da6a3ce929d0e0e473-00f067aa0ba902b7-01",
            "00-+bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7",
            "0g-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7",
        ] {
            assert_eq!(TraceParent::parse(header), None, "{header}");
        }
    }

    #[test]
    fn unknown_flags_are_dropped_and_only_sampled_is_kept() {
        let parsed =
            TraceParent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-ff").unwrap();

        assert_eq!(parsed.flags, 0x01);
        assert!(parsed.to_string().ends_with("-01"));
    }

    #[test]
    fn a_later_version_is_read_by_its_first_four_fields() {
        for header in [
            "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "cc-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-what-the-future-holds",
        ] {
            let parsed = TraceParent::parse(header).unwrap();

            assert_eq!(parsed.to_string(), VALID, "{header}");
        }
    }

    #[test]
    fn a_child_keeps_the_trace_and_takes_a_new_span() {
        let parent = TraceParent::root();
        let child = parent.child();

        assert_eq!(child.trace_id, parent.trace_id);
        assert_ne!(child.span_id, parent.span_id);
        assert_eq!(TraceParent::parse(&child.to_string()), Some(child));
    }
}
