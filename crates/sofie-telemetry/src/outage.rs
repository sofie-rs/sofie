use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy)]
pub(crate) enum Output {
    Logs = 0,
    #[cfg(feature = "otlp")]
    Spans = 1,
    #[cfg(all(feature = "otlp", feature = "metrics"))]
    Metrics = 2,
    #[cfg(feature = "otlp")]
    ExportedLogs = 3,
}

const NAMES: [&str; 4] = ["logs", "spans", "metrics", "exported logs"];
static FAILING: [AtomicBool; 4] = [const { AtomicBool::new(false) }; 4];

#[cfg(feature = "otlp")]
impl Output {
    pub(crate) fn name(self) -> &'static str {
        NAMES[self as usize]
    }
}

/// True when this report is the one that found the output failing.
pub(crate) fn report(output: Output, delivered: bool) -> bool {
    let was_failing = FAILING[output as usize].swap(!delivered, Ordering::Relaxed);
    !delivered && !was_failing
}

/// Readiness reports it as degraded: a telemetry outage never takes an instance out of rotation.
#[must_use]
pub fn outage() -> Option<&'static str> {
    FAILING
        .iter()
        .position(|failing| failing.load(Ordering::Relaxed))
        .map(|index| NAMES[index])
}

#[cfg(feature = "otlp")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_output_is_out_until_it_delivers_again() {
        report(Output::Spans, false);
        assert_eq!(outage(), Some("spans"));

        report(Output::Spans, true);
        assert_eq!(outage(), None);
    }
}
