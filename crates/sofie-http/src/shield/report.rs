use super::detector::Decision;

#[cfg(feature = "metrics")]
static DECISIONS: sofie_telemetry::metrics::Instrument = sofie_telemetry::metrics::Instrument {
    name: "sofie.shield.decisions",
    description: "The shield's decisions, by rule and grade; dry runs included.",
    kind: sofie_telemetry::metrics::Kind::Counter,
    labels: &["shield.rule", "shield.grade"],
};
#[cfg(feature = "metrics")]
static DROPPED: sofie_telemetry::metrics::Instrument = sofie_telemetry::metrics::Instrument {
    name: "sofie.shield.signals.dropped",
    description: "Signals dropped because the shield's queue or its windows were full.",
    kind: sofie_telemetry::metrics::Kind::Counter,
    labels: &[],
};

pub(crate) fn decided(decision: &Decision, dry_run: bool) {
    tracing::warn!(
        shield.rule = decision.rule.name(),
        shield.grade = decision.grade.name(),
        shield.reason = decision.rule.reason(),
        shield.caller = %decision.key.shown(),
        shield.count = decision.count,
        shield.threshold = decision.threshold,
        shield.until = %instant(decision.until),
        shield.dry_run = dry_run,
        area = decision.area.unwrap_or_default(),
        "the shield decided"
    );
    #[cfg(feature = "metrics")]
    sofie_telemetry::metrics::record(
        &DECISIONS,
        &[decision.rule.name().into(), decision.grade.name().into()],
        1.0,
    );
}

pub(crate) fn instant(micros: i64) -> String {
    sofie_core::Timestamp::from_microsecond(micros)
        .map_or_else(|_| micros.to_string(), |at| at.to_string())
}

pub(crate) fn table_full(decision: &Decision) {
    tracing::warn!(
        shield.rule = decision.rule.name(),
        shield.grade = decision.grade.name(),
        "the shield's block table is full; the decision is reported only"
    );
}

pub(crate) fn dropped(count: u64) {
    if count == 0 {
        return;
    }
    tracing::warn!(
        shield.dropped = count,
        "the shield dropped signals it had no room for"
    );
    #[cfg(feature = "metrics")]
    #[expect(
        clippy::cast_precision_loss,
        reason = "a count of dropped signals is far below 2^52"
    )]
    sofie_telemetry::metrics::record(&DROPPED, &[], count as f64);
}
