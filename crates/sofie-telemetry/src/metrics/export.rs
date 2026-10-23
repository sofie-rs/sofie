//! The aggregates `/metrics` renders, sent as cumulative `ExportMetricsServiceRequest`s.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::oneshot;

use super::aggregate::{BOUNDS, Family, Histogram, State};
use super::{Kind, Label, Labels, Message, RECORDER};
use crate::InitError;
use crate::otlp::batch::{self, Closing, Queue, Signal};
use crate::otlp::common::{self, Value};
use crate::otlp::protobuf::Encoder;
use crate::otlp::settings::Settings;
use crate::outage::Output;

const SCOPE: &str = "sofie";
/// The SDK periodic reader's default.
const INTERVAL: Duration = Duration::from_secs(60);
const SNAPSHOT_WAIT: Duration = Duration::from_secs(2);

static SNAPSHOTS: OnceLock<Arc<Queue<Vec<u8>>>> = OnceLock::new();

mod request {
    pub(super) const RESOURCE_METRICS: u32 = 1;
    pub(super) const SCOPE_METRICS: u32 = 2;
    pub(super) const METRICS: u32 = 2;
}

mod metric {
    pub(super) const NAME: u32 = 1;
    pub(super) const DESCRIPTION: u32 = 2;
    pub(super) const UNIT: u32 = 3;
    pub(super) const GAUGE: u32 = 5;
    pub(super) const SUM: u32 = 7;
    pub(super) const HISTOGRAM: u32 = 9;
    pub(super) const DATA_POINTS: u32 = 1;
    pub(super) const AGGREGATION_TEMPORALITY: u32 = 2;
    pub(super) const IS_MONOTONIC: u32 = 3;
    /// `AggregationTemporality.CUMULATIVE`, the SDK's default.
    pub(super) const CUMULATIVE: u64 = 2;
}

mod number {
    pub(super) const START_TIME_UNIX_NANO: u32 = 2;
    pub(super) const TIME_UNIX_NANO: u32 = 3;
    pub(super) const AS_DOUBLE: u32 = 4;
    pub(super) const ATTRIBUTES: u32 = 7;
}

mod histogram {
    pub(super) const START_TIME_UNIX_NANO: u32 = 2;
    pub(super) const TIME_UNIX_NANO: u32 = 3;
    pub(super) const COUNT: u32 = 4;
    pub(super) const SUM: u32 = 5;
    pub(super) const BUCKET_COUNTS: u32 = 6;
    pub(super) const EXPLICIT_BOUNDS: u32 = 7;
    pub(super) const ATTRIBUTES: u32 = 9;
}

/// Each snapshot is cumulative, so a newer one replaces one still waiting.
struct Snapshots;

impl Signal for Snapshots {
    type Item = Vec<u8>;
    const PATH: &'static str = "/v1/metrics";
    const OUTPUT: Output = Output::Metrics;
    const CAPACITY: usize = 1;
    const BATCH_SIZE: usize = 1;
    const INTERVAL: Duration = INTERVAL;

    fn encode(&self, mut snapshots: Vec<Vec<u8>>, _: &[u8]) -> Vec<u8> {
        snapshots.pop().unwrap_or_default()
    }
}

#[derive(Clone, Copy)]
struct Times {
    start: u64,
    now: u64,
}

/// Owned by the metrics thread, which encodes a snapshot when one is due.
pub(super) struct Exporter {
    snapshots: Arc<Queue<Vec<u8>>>,
    settings: Arc<Settings>,
    started: SystemTime,
    next: Instant,
}

impl Exporter {
    pub(super) fn exporting() -> Result<Option<Self>, InitError> {
        let Some(settings) = Settings::from_environment()? else {
            return Ok(None);
        };
        let settings = Arc::new(settings);
        let snapshots = batch::start(Snapshots, Arc::clone(&settings));
        SNAPSHOTS
            .set(Arc::clone(&snapshots))
            .map_err(|_| InitError::new("the OTLP metric exporter was already installed"))?;
        Ok(Some(Self {
            snapshots,
            settings,
            started: SystemTime::now(),
            next: Instant::now() + INTERVAL,
        }))
    }

    pub(super) fn export_when_due(&mut self, families: &BTreeMap<&'static str, Family>) {
        if Instant::now() >= self.next {
            self.export(families);
        }
    }

    pub(super) fn export(&mut self, families: &BTreeMap<&'static str, Family>) {
        self.next = Instant::now() + INTERVAL;
        let times = Times {
            start: common::unix_nanos(self.started),
            now: common::unix_nanos(SystemTime::now()),
        };
        self.snapshots
            .push(encode(families, &self.settings.resource, times));
    }
}

/// Takes a last snapshot first, so the close exports it.
pub(crate) async fn close(deadline: tokio::time::Instant) -> Option<Closing> {
    let snapshots = SNAPSHOTS.get()?;
    if let Some(recorder) = RECORDER.get() {
        let (done, taken) = oneshot::channel();
        if recorder.send(Message::Export(done)).is_ok() {
            let waited = deadline.min(tokio::time::Instant::now() + SNAPSHOT_WAIT);
            let _ = tokio::time::timeout_at(waited, taken).await;
        }
    }
    Some(snapshots.close())
}

fn encode(families: &BTreeMap<&'static str, Family>, resource: &[u8], times: Times) -> Vec<u8> {
    let mut encoder = Encoder::default();
    encoder.message(request::RESOURCE_METRICS, |encoder| {
        common::resource(encoder, resource);
        encoder.message(request::SCOPE_METRICS, |encoder| {
            common::scope(encoder, SCOPE);
            for family in families.values() {
                if family.read.is_some() || !family.series.is_empty() {
                    encoder.message(request::METRICS, |encoder| {
                        encode_metric(encoder, family, times);
                    });
                }
            }
        });
    });
    encoder.into_bytes()
}

fn encode_metric(encoder: &mut Encoder, family: &Family, times: Times) {
    let instrument = family.instrument;
    encoder.string(metric::NAME, instrument.name);
    if !instrument.description.is_empty() {
        encoder.string(metric::DESCRIPTION, instrument.description);
    }
    match instrument.kind {
        Kind::Duration => {
            encoder.string(metric::UNIT, "s");
            encoder.message(metric::HISTOGRAM, |encoder| {
                for (labels, state) in sorted_series(family) {
                    if let State::Histogram(histogram) = state {
                        encoder.message(metric::DATA_POINTS, |encoder| {
                            histogram_point(encoder, family, labels, histogram, times);
                        });
                    }
                }
                encoder.varint(metric::AGGREGATION_TEMPORALITY, metric::CUMULATIVE);
            });
        }
        Kind::Counter => encoder.message(metric::SUM, |encoder| {
            number_points(encoder, family, times);
            encoder.varint(metric::AGGREGATION_TEMPORALITY, metric::CUMULATIVE);
            encoder.bool(metric::IS_MONOTONIC, true);
        }),
        Kind::Gauge => encoder.message(metric::GAUGE, |encoder| {
            number_points(encoder, family, times);
        }),
    }
}

fn number_points(encoder: &mut Encoder, family: &Family, times: Times) {
    if let Some(read) = family.read {
        encoder.message(metric::DATA_POINTS, |encoder| {
            encoder.fixed64(number::TIME_UNIX_NANO, times.now);
            encoder.double(number::AS_DOUBLE, read());
        });
    }
    for (labels, state) in sorted_series(family) {
        if let State::Total(total) = state {
            encoder.message(metric::DATA_POINTS, |encoder| {
                encoder.fixed64(number::START_TIME_UNIX_NANO, times.start);
                encoder.fixed64(number::TIME_UNIX_NANO, times.now);
                encoder.double(number::AS_DOUBLE, *total);
                attributes(encoder, number::ATTRIBUTES, family, labels);
            });
        }
    }
}

fn histogram_point(
    encoder: &mut Encoder,
    family: &Family,
    labels: &Labels,
    histogram: &Histogram,
    times: Times,
) {
    let mut bucket_counts = [0; BOUNDS.len() + 1];
    bucket_counts[..BOUNDS.len()].copy_from_slice(&histogram.buckets);
    bucket_counts[BOUNDS.len()] = histogram.count - histogram.buckets.iter().sum::<u64>();
    encoder.fixed64(histogram::START_TIME_UNIX_NANO, times.start);
    encoder.fixed64(histogram::TIME_UNIX_NANO, times.now);
    encoder.fixed64(histogram::COUNT, histogram.count);
    encoder.double(histogram::SUM, histogram.sum);
    encoder.packed_fixed64(histogram::BUCKET_COUNTS, &bucket_counts);
    encoder.packed_double(histogram::EXPLICIT_BOUNDS, &BOUNDS);
    attributes(encoder, histogram::ATTRIBUTES, family, labels);
}

fn attributes(encoder: &mut Encoder, field: u32, family: &Family, labels: &Labels) {
    for (key, label) in family.instrument.labels.iter().zip(labels) {
        let value = match label {
            Label::Name(name) => Value::Text(Cow::Borrowed(name)),
            Label::Code(code) => Value::Int(i64::from(*code)),
        };
        common::key_value(encoder, field, key, &value);
    }
}

fn sorted_series(family: &Family) -> Vec<(&Labels, &State)> {
    let mut series: Vec<_> = family.series.iter().collect();
    series.sort_by_key(|(labels, _)| **labels);
    series
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use super::*;
    use crate::metrics::Instrument;

    static ORDERS: Instrument = Instrument {
        name: "test.orders",
        description: "",
        kind: Kind::Counter,
        labels: &["k"],
    };
    static RENDERS: Instrument = Instrument {
        name: "test.renders",
        description: "",
        kind: Kind::Duration,
        labels: &[],
    };

    fn labels(first: Label) -> Labels {
        [first, Label::Name(""), Label::Name(""), Label::Name("")]
    }

    #[test]
    fn a_counter_encodes_to_the_bytes_the_proto_definitions_give() {
        let mut families = BTreeMap::new();
        families.insert(
            "test.orders",
            Family {
                instrument: &ORDERS,
                series: HashMap::from([(labels(Label::Name("v")), State::Total(2.0))]),
                read: None,
            },
        );

        let bytes = encode(&families, &[], Times { start: 1, now: 2 });

        #[rustfmt::skip]
        let expected: &[u8] = &[
            0x0a, 0x49, // resource_metrics, 73 bytes
              0x0a, 0x00, // resource: the given, empty body
              0x12, 0x45, // scope_metrics, 69 bytes
                0x0a, 0x07, 0x0a, 0x05, b's', b'o', b'f', b'i', b'e', // scope { name }
                0x12, 0x3a, // metrics, 58 bytes
                  0x0a, 0x0b, b't', b'e', b's', b't', b'.', b'o', b'r', b'd', b'e', b'r', b's', // name
                  0x3a, 0x2b, // sum, 43 bytes
                    0x0a, 0x25, // data_points, 37 bytes
                      0x11, 0x01, 0, 0, 0, 0, 0, 0, 0, // start_time_unix_nano
                      0x19, 0x02, 0, 0, 0, 0, 0, 0, 0, // time_unix_nano
                      0x21, 0, 0, 0, 0, 0, 0, 0, 0x40, // as_double: 2.0
                      0x3a, 0x08, 0x0a, 0x01, b'k', 0x12, 0x03, 0x0a, 0x01, b'v', // attributes
                    0x10, 0x02, // aggregation_temporality: cumulative
                    0x18, 0x01, // is_monotonic
        ];
        assert_eq!(bytes, expected);
    }

    #[test]
    fn a_duration_carries_every_bucket_and_one_above_the_last_bound() {
        let mut histogram = Histogram::default();
        histogram.add(0.003);
        histogram.add(60.0);
        let mut families = BTreeMap::new();
        families.insert(
            "test.renders",
            Family {
                instrument: &RENDERS,
                series: HashMap::from([(labels(Label::Name("")), State::Histogram(histogram))]),
                read: None,
            },
        );

        let bytes = encode(&families, &[], Times { start: 1, now: 2 });

        let mut counts = vec![0x32, 0x60]; // bucket_counts: 12 fixed64s
        counts.extend(1_u64.to_le_bytes());
        counts.extend([0; 80]);
        counts.extend(1_u64.to_le_bytes());
        let mut bounds = vec![0x3a, 0x58]; // explicit_bounds: 11 doubles
        bounds.extend(BOUNDS.iter().flat_map(|bound| bound.to_le_bytes()));
        let count = [0x21, 2, 0, 0, 0, 0, 0, 0, 0];
        for part in [
            &counts[..],
            &bounds[..],
            &count[..],
            &[0x1a, 0x01, b's'][..],
        ] {
            assert!(
                bytes.windows(part.len()).any(|window| window == part),
                "{part:02x?} in {bytes:02x?}"
            );
        }
    }
}
