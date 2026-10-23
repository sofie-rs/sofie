#![cfg(feature = "metrics")]

use std::time::Duration;

use sofie_telemetry::metrics::{self, Instrument, Kind, Label};

static RENDERS: Instrument = Instrument {
    name: "test.render.duration",
    description: "How long a test render took.",
    kind: Kind::Duration,
    labels: &["screen", "http.response.status_code"],
};
static OPEN: Instrument = Instrument {
    name: "test.open",
    description: "What is open.",
    kind: Kind::Gauge,
    labels: &[],
};

/// One worker thread: a scrape that blocked it would keep the spawned task from running.
#[tokio::test]
async fn measurements_and_gauges_are_scraped_once_started_without_blocking() {
    assert_eq!(metrics::prometheus().await, None);
    metrics::start().unwrap();

    let labels = [Label::from("order/web/show"), Label::from(200_u16)];
    metrics::record_duration(&RENDERS, &labels, Duration::from_millis(30));
    metrics::gauge(&OPEN, || 7.0);
    let meanwhile = tokio::spawn(async {});
    let text = metrics::prometheus().await.unwrap();

    assert!(meanwhile.is_finished());

    let series = "screen=\"order/web/show\",http_response_status_code=\"200\"";
    assert!(
        text.contains(&format!(
            "test_render_duration_seconds_bucket{{{series},le=\"0.05\"}} 1\n"
        )),
        "{text}"
    );
    assert!(
        text.contains("# TYPE test_open gauge\ntest_open 7\n"),
        "{text}"
    );
}
