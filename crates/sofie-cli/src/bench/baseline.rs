use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

const LATENCY_MARGIN: f64 = 0.2;
const ERROR_RATE_MARGIN: f64 = 0.01;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measurement {
    pub p95_ms: f64,
    pub error_rate: f64,
}

pub type Measurements = BTreeMap<String, Measurement>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    New,
    Held,
    Lowered,
    Regressed(Vec<String>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RouteResult {
    pub endpoint: String,
    pub measured: Measurement,
    pub baseline: Option<Measurement>,
    pub verdict: Verdict,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comparison {
    pub routes: Vec<RouteResult>,
    pub next_baseline: Measurements,
}

impl Comparison {
    #[must_use]
    pub fn regressed(&self) -> bool {
        self.routes
            .iter()
            .any(|route| matches!(route.verdict, Verdict::Regressed(_)))
    }
}

/// A ceiling: a regression beyond the margin fails and keeps the old value,
/// an improvement lowers it, an endpoint no longer measured drops out.
#[must_use]
pub fn compare(baseline: &Measurements, measured: &Measurements) -> Comparison {
    let mut routes = Vec::new();
    let mut next_baseline = Measurements::new();
    for (endpoint, &measured) in measured {
        let previous = baseline.get(endpoint).copied();
        let (verdict, ceiling) = previous.map_or((Verdict::New, measured), |previous| {
            judge(previous, measured)
        });
        next_baseline.insert(endpoint.clone(), ceiling);
        routes.push(RouteResult {
            endpoint: endpoint.clone(),
            measured,
            baseline: previous,
            verdict,
        });
    }
    Comparison {
        routes,
        next_baseline,
    }
}

fn judge(previous: Measurement, measured: Measurement) -> (Verdict, Measurement) {
    let mut regressions = Vec::new();
    if measured.p95_ms > previous.p95_ms * (1.0 + LATENCY_MARGIN) {
        regressions.push(format!(
            "p95 {:.2} ms is above {:.2} ms plus {:.0}%",
            measured.p95_ms,
            previous.p95_ms,
            LATENCY_MARGIN * 100.0
        ));
    }
    if measured.error_rate > previous.error_rate + ERROR_RATE_MARGIN {
        regressions.push(format!(
            "error rate {:.2}% is above {:.2}% plus {:.0} points",
            measured.error_rate * 100.0,
            previous.error_rate * 100.0,
            ERROR_RATE_MARGIN * 100.0
        ));
    }
    if !regressions.is_empty() {
        return (Verdict::Regressed(regressions), previous);
    }
    let ceiling = Measurement {
        p95_ms: previous.p95_ms.min(measured.p95_ms),
        error_rate: previous.error_rate.min(measured.error_rate),
    };
    let verdict = if ceiling == previous {
        Verdict::Held
    } else {
        Verdict::Lowered
    };
    (verdict, ceiling)
}

/// The shape of both `bench/baseline.json` and the summary the k6 script
/// writes: `{"routes": {"<endpoint>": {"p95_ms": …, "error_rate": …}}}`.
pub fn parse(text: &str) -> Result<Measurements, String> {
    let document: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let routes = document
        .get("routes")
        .and_then(Value::as_object)
        .ok_or("no `routes` object")?;
    routes
        .iter()
        .map(|(endpoint, values)| {
            let number = |key: &str| {
                values
                    .get(key)
                    .and_then(Value::as_f64)
                    .ok_or_else(|| format!("`{endpoint}` has no number `{key}`"))
            };
            let measurement = Measurement {
                p95_ms: round(number("p95_ms")?, 100.0),
                error_rate: round(number("error_rate")?, 10_000.0),
            };
            Ok((endpoint.clone(), measurement))
        })
        .collect()
}

#[must_use]
pub fn render(measurements: &Measurements) -> String {
    let routes: Map<String, Value> = measurements
        .iter()
        .map(|(endpoint, measurement)| {
            let values =
                json!({ "p95_ms": measurement.p95_ms, "error_rate": measurement.error_rate });
            (endpoint.clone(), values)
        })
        .collect();
    let text = serde_json::to_string_pretty(&json!({ "routes": routes }))
        .expect("a map of numbers serializes");
    format!("{text}\n")
}

fn round(value: f64, scale: f64) -> f64 {
    (value * scale).round() / scale
}

#[cfg(test)]
mod tests {
    use super::{Measurement, Measurements, Verdict, compare, parse, render};

    fn measurements(entries: &[(&str, f64, f64)]) -> Measurements {
        entries
            .iter()
            .map(|&(endpoint, p95_ms, error_rate)| {
                let measurement = Measurement { p95_ms, error_rate };
                (endpoint.to_owned(), measurement)
            })
            .collect()
    }

    #[test]
    fn a_regression_beyond_the_margin_fails_and_keeps_the_old_ceiling() {
        let baseline = measurements(&[("list html", 10.0, 0.0)]);

        let comparison = compare(&baseline, &measurements(&[("list html", 12.5, 0.0)]));

        assert!(comparison.regressed());
        assert!(
            matches!(&comparison.routes[0].verdict, Verdict::Regressed(reasons) if reasons[0].contains("p95 12.50 ms"))
        );
        assert_eq!(comparison.next_baseline, baseline);
    }

    #[test]
    fn an_error_rate_above_the_margin_is_a_regression() {
        let baseline = measurements(&[("list json", 10.0, 0.0)]);

        let comparison = compare(&baseline, &measurements(&[("list json", 9.0, 0.02)]));

        assert!(comparison.regressed());
    }

    #[test]
    fn noise_within_the_margin_holds_and_an_improvement_lowers_the_ceiling() {
        let baseline = measurements(&[("list html", 10.0, 0.0), ("list json", 8.0, 0.01)]);

        let comparison = compare(
            &baseline,
            &measurements(&[("list html", 11.5, 0.0), ("list json", 6.0, 0.0)]),
        );

        assert!(!comparison.regressed());
        assert_eq!(comparison.routes[0].verdict, Verdict::Held);
        assert_eq!(comparison.routes[1].verdict, Verdict::Lowered);
        assert_eq!(
            comparison.next_baseline,
            measurements(&[("list html", 10.0, 0.0), ("list json", 6.0, 0.0)])
        );
    }

    #[test]
    fn a_new_endpoint_sets_its_ceiling_and_a_removed_one_drops_out() {
        let baseline = measurements(&[("gone json", 3.0, 0.0)]);

        let comparison = compare(&baseline, &measurements(&[("list json", 4.0, 0.0)]));

        assert_eq!(comparison.routes[0].verdict, Verdict::New);
        assert_eq!(
            comparison.next_baseline,
            measurements(&[("list json", 4.0, 0.0)])
        );
    }

    #[test]
    fn the_baseline_file_reads_back_what_it_wrote_rounded() {
        let summary = r#"{"routes": {"list html": {"p95_ms": 12.3456, "error_rate": 0.000049}}}"#;

        let read = parse(summary).unwrap();

        assert_eq!(read, measurements(&[("list html", 12.35, 0.0)]));
        assert_eq!(parse(&render(&read)).unwrap(), read);
    }
}
