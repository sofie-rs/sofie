use std::time::{Duration, Instant};

use sofie_core::Result;
use tracing::field::Empty;
use tracing::{Instrument, Span};

const SLOW_STATEMENT: Duration = Duration::from_millis(250);

/// Outside traced work no span opens, or the worker's polling would trace every second.
pub async fn traced_statement<T>(
    sql: &str,
    run: impl Future<Output = Result<T>>,
    rows: impl FnOnce(&T) -> Option<u64>,
) -> Result<T> {
    let span = if Span::current().is_disabled() {
        Span::none()
    } else {
        tracing::info_span!("statement", db.query.text = sql, db.rows = Empty)
    };
    let started = Instant::now();
    let result = run.instrument(span.clone()).await;
    if let Some(count) = result.as_ref().ok().and_then(rows) {
        span.record("db.rows", count);
    }
    let elapsed = started.elapsed();
    #[cfg(feature = "metrics")]
    crate::metrics::statement(sql, elapsed);
    if elapsed > SLOW_STATEMENT {
        let duration_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        span.in_scope(|| tracing::warn!(db.query.text = sql, duration_ms, "slow statement"));
    }
    result
}
