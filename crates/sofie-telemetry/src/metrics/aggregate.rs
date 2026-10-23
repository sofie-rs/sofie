use std::collections::{BTreeMap, HashMap};

use super::{Instrument, Kind, Label, Labels, Measurement, Message};
use crate::InitError;
use crate::fields::push_formatted;

/// Prometheus' default bounds, in seconds.
pub(super) const BOUNDS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

pub(super) enum State {
    Total(f64),
    Histogram(Histogram),
}

#[derive(Default)]
pub(super) struct Histogram {
    /// Per bucket, not yet cumulative; a value above every bound is only counted.
    pub(super) buckets: [u64; BOUNDS.len()],
    pub(super) count: u64,
    pub(super) sum: f64,
}

pub(super) struct Family {
    pub(super) instrument: &'static Instrument,
    pub(super) series: HashMap<Labels, State>,
    pub(super) read: Option<fn() -> f64>,
}

pub(super) struct Aggregates {
    families: BTreeMap<&'static str, Family>,
    polls: Vec<Box<dyn Fn() + Send>>,
    #[cfg(feature = "otlp")]
    export: Option<super::export::Exporter>,
}

impl Aggregates {
    #[cfg_attr(
        not(feature = "otlp"),
        expect(
            clippy::unnecessary_wraps,
            reason = "only the exporter can fail to start"
        )
    )]
    pub(super) fn new() -> Result<Self, InitError> {
        Ok(Self {
            families: BTreeMap::new(),
            polls: Vec::new(),
            #[cfg(feature = "otlp")]
            export: super::export::Exporter::exporting()?,
        })
    }

    pub(super) fn receive(&mut self, message: Message) {
        match message {
            Message::Measured(measurement) => self.add(&measurement),
            Message::Gauge(instrument, read) => self.family(instrument).read = Some(read),
            // A scrape that gave up waiting no longer listens.
            Message::Render(answer) => drop(answer.send(self.prometheus())),
            Message::Poll(poll) => {
                poll();
                self.polls.push(poll);
            }
            #[cfg(feature = "otlp")]
            Message::Export(done) => {
                if let Some(export) = &mut self.export {
                    export.export(&self.families);
                }
                // The shutdown may have stopped waiting.
                let _ = done.send(());
            }
        }
    }

    #[cfg(feature = "otlp")]
    pub(super) fn export_when_due(&mut self) {
        if let Some(export) = &mut self.export {
            export.export_when_due(&self.families);
        }
    }

    pub(super) fn poll(&self) {
        self.polls.iter().for_each(|poll| poll());
    }

    /// A value that is not finite would stay in a sum for the life of the process.
    fn add(&mut self, measurement: &Measurement) {
        if !measurement.value.is_finite() {
            return;
        }
        let kind = measurement.instrument.kind;
        let state = self
            .family(measurement.instrument)
            .series
            .entry(measurement.labels)
            .or_insert_with(|| match kind {
                Kind::Duration => State::Histogram(Histogram::default()),
                Kind::Counter | Kind::Gauge => State::Total(0.0),
            });
        match state {
            State::Total(total) => *total += measurement.value,
            State::Histogram(histogram) => histogram.add(measurement.value),
        }
    }

    fn family(&mut self, instrument: &'static Instrument) -> &mut Family {
        self.families
            .entry(instrument.name)
            .or_insert_with(|| Family {
                instrument,
                series: HashMap::new(),
                read: None,
            })
    }

    pub(super) fn prometheus(&self) -> String {
        let mut text = String::new();
        for family in self.families.values() {
            family.push_prometheus(&mut text);
        }
        text
    }
}

impl Histogram {
    pub(super) fn add(&mut self, seconds: f64) {
        if let Some(bucket) = BOUNDS.iter().position(|bound| seconds <= *bound) {
            self.buckets[bucket] += 1;
        }
        self.count += 1;
        self.sum += seconds;
    }
}

impl Family {
    fn push_prometheus(&self, text: &mut String) {
        let instrument = self.instrument;
        let name = prometheus_name(instrument);
        let kind = match instrument.kind {
            Kind::Counter => "counter",
            Kind::Duration => "histogram",
            Kind::Gauge => "gauge",
        };
        push_formatted(
            text,
            format_args!(
                "# HELP {name} {}\n# TYPE {name} {kind}\n",
                instrument.description
            ),
        );
        if let Some(read) = self.read {
            push_formatted(text, format_args!("{name} {}\n", read()));
        }
        let mut series: Vec<_> = self.series.iter().collect();
        series.sort_by_key(|(labels, _)| **labels);
        for (labels, state) in series {
            let labels = render_labels(instrument, labels);
            match state {
                State::Total(total) => push_sample(text, &name, "", &labels, None, *total),
                State::Histogram(histogram) => histogram.push_prometheus(text, &name, &labels),
            }
        }
    }
}

impl Histogram {
    fn push_prometheus(&self, text: &mut String, name: &str, labels: &str) {
        let mut cumulative = 0;
        for (bound, count) in BOUNDS.iter().zip(self.buckets) {
            cumulative += count;
            let bound = bound.to_string();
            push_count(text, name, labels, Some(&bound), cumulative);
        }
        push_count(text, name, labels, Some("+Inf"), self.count);
        push_sample(text, name, "_sum", labels, None, self.sum);
        push_count(text, name, labels, None, self.count);
    }
}

fn push_count(text: &mut String, name: &str, labels: &str, bound: Option<&str>, count: u64) {
    let suffix = if bound.is_some() { "_bucket" } else { "_count" };
    #[expect(
        clippy::cast_precision_loss,
        reason = "Prometheus samples are floats; counts this large are not reached"
    )]
    push_sample(text, name, suffix, labels, bound, count as f64);
}

fn push_sample(
    text: &mut String,
    name: &str,
    suffix: &str,
    labels: &str,
    bound: Option<&str>,
    value: f64,
) {
    let mut all = labels.to_owned();
    if let Some(bound) = bound {
        if !all.is_empty() {
            all.push(',');
        }
        push_formatted(&mut all, format_args!("le=\"{bound}\""));
    }
    if all.is_empty() {
        push_formatted(text, format_args!("{name}{suffix} {value}\n"));
    } else {
        push_formatted(text, format_args!("{name}{suffix}{{{all}}} {value}\n"));
    }
}

fn prometheus_name(instrument: &Instrument) -> String {
    let suffix = match instrument.kind {
        Kind::Counter => "_total",
        Kind::Duration => "_seconds",
        Kind::Gauge => "",
    };
    format!("{}{suffix}", instrument.name.replace('.', "_"))
}

fn render_labels(instrument: &Instrument, labels: &Labels) -> String {
    let mut text = String::new();
    for (key, label) in instrument.labels.iter().zip(labels) {
        if !text.is_empty() {
            text.push(',');
        }
        text.push_str(&key.replace('.', "_"));
        text.push_str("=\"");
        match label {
            Label::Name(name) => push_escaped(&mut text, name),
            Label::Code(code) => push_formatted(&mut text, format_args!("{code}")),
        }
        text.push('"');
    }
    text
}

fn push_escaped(text: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '\\' => text.push_str("\\\\"),
            '"' => text.push_str("\\\""),
            '\n' => text.push_str("\\n"),
            other => text.push(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUESTS: Instrument = Instrument {
        name: "test.request.duration",
        description: "How long a request took.",
        kind: Kind::Duration,
        labels: &["http.route", "http.response.status_code"],
    };
    const EVENTS: Instrument = Instrument {
        name: "test.events",
        description: "Events seen.",
        kind: Kind::Counter,
        labels: &["name"],
    };

    fn measured(instrument: &'static Instrument, labels: &[Label], value: f64) -> Message {
        let mut placed = [Label::Name(""); super::super::MAX_LABELS];
        placed[..labels.len()].copy_from_slice(labels);
        Message::Measured(Measurement {
            instrument,
            labels: placed,
            value,
        })
    }

    #[test]
    fn durations_render_as_cumulative_buckets_and_counters_as_totals() {
        let mut aggregates = Aggregates {
            families: BTreeMap::new(),
            polls: Vec::new(),
            #[cfg(feature = "otlp")]
            export: None,
        };
        let route = [Label::Name("order.web.show"), Label::Code(200)];
        aggregates.receive(measured(&REQUESTS, &route, 0.003));
        aggregates.receive(measured(&REQUESTS, &route, 0.2));
        aggregates.receive(measured(&EVENTS, &[Label::Name("say \"hi\"")], 1.0));
        aggregates.receive(measured(&EVENTS, &[Label::Name("say \"hi\"")], 1.0));

        let text = aggregates.prometheus();

        assert!(
            text.contains("# TYPE test_events_total counter\n"),
            "{text}"
        );
        assert!(
            text.contains("test_events_total{name=\"say \\\"hi\\\"\"} 2\n"),
            "{text}"
        );
        let labels = "http_route=\"order.web.show\",http_response_status_code=\"200\"";
        for sample in [
            format!("test_request_duration_seconds_bucket{{{labels},le=\"0.005\"}} 1\n"),
            format!("test_request_duration_seconds_bucket{{{labels},le=\"0.1\"}} 1\n"),
            format!("test_request_duration_seconds_bucket{{{labels},le=\"0.25\"}} 2\n"),
            format!("test_request_duration_seconds_bucket{{{labels},le=\"+Inf\"}} 2\n"),
            format!("test_request_duration_seconds_count{{{labels}}} 2\n"),
        ] {
            assert!(text.contains(&sample), "{sample} in {text}");
        }
    }

    #[test]
    fn a_value_that_is_not_a_number_is_left_out() {
        let mut aggregates = Aggregates {
            families: BTreeMap::new(),
            polls: Vec::new(),
            #[cfg(feature = "otlp")]
            export: None,
        };
        let route = [Label::Name("order.web.show"), Label::Code(200)];
        aggregates.receive(measured(&REQUESTS, &route, 0.2));
        aggregates.receive(measured(&REQUESTS, &route, f64::NAN));

        let text = aggregates.prometheus();

        assert!(!text.contains("NaN"), "{text}");
        assert!(
            text.contains(
                "_count{http_route=\"order.web.show\",http_response_status_code=\"200\"} 1\n"
            ),
            "{text}"
        );
    }
}
