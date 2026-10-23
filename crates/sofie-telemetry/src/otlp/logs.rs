//! Log lines, as `ExportLogsServiceRequest`s.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use tracing::Level;

use super::batch::{self, Queue, Signal};
use super::common::{self, Attribute, Value};
use super::protobuf::Encoder;
use super::settings::Settings;
use crate::InitError;
use crate::TraceParent;
use crate::line::PendingLine;
use crate::outage::Output;

const MESSAGE: &str = "message";

/// The process's log export, closed at shutdown after the writer hands over its last lines.
pub(super) static LOGS: OnceLock<Arc<Queue<LogRecord>>> = OnceLock::new();

mod request {
    pub(super) const RESOURCE_LOGS: u32 = 1;
    pub(super) const SCOPE_LOGS: u32 = 2;
    pub(super) const LOG_RECORDS: u32 = 2;
}

mod record {
    pub(super) const TIME_UNIX_NANO: u32 = 1;
    pub(super) const SEVERITY_NUMBER: u32 = 2;
    pub(super) const SEVERITY_TEXT: u32 = 3;
    pub(super) const BODY: u32 = 5;
    pub(super) const ATTRIBUTES: u32 = 6;
    pub(super) const FLAGS: u32 = 8;
    pub(super) const TRACE_ID: u32 = 9;
    pub(super) const SPAN_ID: u32 = 10;
    pub(super) const OBSERVED_TIME_UNIX_NANO: u32 = 11;
}

pub(crate) struct LogRecord {
    time: SystemTime,
    observed: SystemTime,
    level: Level,
    /// Exported as the record's scope, as the SDK did.
    target: &'static str,
    trace: Option<TraceParent>,
    body: Option<Value>,
    attributes: Vec<Attribute>,
}

impl LogRecord {
    fn of(line: PendingLine) -> Self {
        let mut body = None;
        let mut attributes = Vec::new();
        for (name, value) in line.fields {
            if name == MESSAGE {
                body = Some(Value::from(value));
            } else {
                attributes.push(Attribute::new(name, Value::from(value)));
            }
        }
        Self {
            time: line.time,
            observed: SystemTime::now(),
            level: line.level,
            target: line.target,
            trace: line.trace,
            body,
            attributes,
        }
    }
}

struct Logs;

impl Signal for Logs {
    type Item = LogRecord;
    const PATH: &'static str = "/v1/logs";
    const OUTPUT: Output = Output::ExportedLogs;
    const CAPACITY: usize = 2048;
    const BATCH_SIZE: usize = 512;
    const INTERVAL: Duration = Duration::from_secs(1);

    fn encode(&self, mut records: Vec<LogRecord>, resource: &[u8]) -> Vec<u8> {
        // Stable: each target's records keep their order.
        records.sort_by_key(|record| record.target);
        encode(&records, resource)
    }
}

/// Owned by the log writer's thread; a task on the runtime batches and exports the records.
pub(crate) struct LogExport {
    records: Arc<Queue<LogRecord>>,
}

impl LogExport {
    pub(crate) fn exporting() -> Result<Option<Self>, InitError> {
        let Some(settings) = Settings::from_environment()? else {
            return Ok(None);
        };
        let export = Self::start(Arc::new(settings));
        LOGS.set(Arc::clone(&export.records))
            .map_err(|_| InitError::new("the OTLP log export was already installed"))?;
        Ok(Some(export))
    }

    /// Must be called inside a Tokio runtime.
    fn start(settings: Arc<Settings>) -> Self {
        Self {
            records: batch::start(Logs, settings),
        }
    }

    pub(crate) fn send(&mut self, line: PendingLine) {
        self.records.push(LogRecord::of(line));
    }

    pub(crate) fn take_dropped(&mut self) -> u64 {
        self.records.take_dropped()
    }
}

/// The records must be sorted by target.
fn encode(records: &[LogRecord], resource: &[u8]) -> Vec<u8> {
    let mut encoder = Encoder::default();
    encoder.message(request::RESOURCE_LOGS, |encoder| {
        common::resource(encoder, resource);
        for same_target in records.chunk_by(|first, second| first.target == second.target) {
            encoder.message(request::SCOPE_LOGS, |encoder| {
                common::scope(encoder, same_target[0].target);
                for log in same_target {
                    encoder.message(request::LOG_RECORDS, |encoder| encode_record(encoder, log));
                }
            });
        }
    });
    encoder.into_bytes()
}

fn encode_record(encoder: &mut Encoder, log: &LogRecord) {
    encoder.fixed64(record::TIME_UNIX_NANO, common::unix_nanos(log.time));
    encoder.varint(record::SEVERITY_NUMBER, severity_number(log.level));
    encoder.string(record::SEVERITY_TEXT, log.level.as_str());
    if let Some(body) = &log.body {
        common::any_value(encoder, record::BODY, body);
    }
    common::attributes(encoder, record::ATTRIBUTES, &log.attributes);
    if let Some(trace) = log.trace {
        if trace.flags != 0 {
            encoder.fixed32(record::FLAGS, u32::from(trace.flags));
        }
        encoder.bytes(record::TRACE_ID, &trace.trace_id.to_be_bytes());
        encoder.bytes(record::SPAN_ID, &trace.span_id.to_be_bytes());
    }
    encoder.fixed64(
        record::OBSERVED_TIME_UNIX_NANO,
        common::unix_nanos(log.observed),
    );
}

fn severity_number(level: Level) -> u64 {
    match level {
        Level::TRACE => 1,
        Level::DEBUG => 5,
        Level::INFO => 9,
        Level::WARN => 13,
        Level::ERROR => 17,
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::sync::Arc;
    use std::time::{Duration, UNIX_EPOCH};

    use tracing::field::Empty;
    use tracing::{Dispatch, Level};

    use super::{LogExport, LogRecord, encode};
    use crate::TraceParent;
    use crate::filter::LogFilter;
    use crate::line::LineFormat;
    use crate::otlp::FINAL_EXPORT_WAIT;
    use crate::otlp::common::{Attribute, Value};
    use crate::otlp::transport::tests::{collector, settings};
    use crate::sample::Sample;
    use crate::subscriber::{Output, Telemetry};
    use crate::writer::LogWriter;
    use crate::writer::tests::{Shared, written};

    const INCOMING: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn a_record_encodes_to_the_bytes_the_proto_definitions_give() {
        let log = LogRecord {
            time: UNIX_EPOCH + Duration::from_nanos(1),
            observed: UNIX_EPOCH + Duration::from_nanos(2),
            level: Level::INFO,
            target: "t",
            trace: Some(TraceParent {
                trace_id: 1,
                span_id: 2,
                flags: 1,
            }),
            body: Some(Value::Text(Cow::Borrowed("hi"))),
            attributes: vec![Attribute::new("n", 7)],
        };

        let bytes = encode(&[log], &[]);

        #[rustfmt::skip]
        let expected: &[u8] = &[
            0x0a, 0x55, // resource_logs, 85 bytes
              0x0a, 0x00, // resource: the given, empty body
              0x12, 0x51, // scope_logs, 81 bytes
                0x0a, 0x03, 0x0a, 0x01, b't', // scope { name: the target }
                0x12, 0x4a, // log_records, 74 bytes
                  0x09, 0x01, 0, 0, 0, 0, 0, 0, 0, // time_unix_nano
                  0x10, 0x09, // severity_number: info
                  0x1a, 0x04, b'I', b'N', b'F', b'O', // severity_text
                  0x2a, 0x04, 0x0a, 0x02, b'h', b'i', // body: string
                  0x32, 0x07, 0x0a, 0x01, b'n', 0x12, 0x02, 0x18, 0x07, // attributes { n: int 7 }
                  0x45, 0x01, 0x00, 0x00, 0x00, // flags: sampled
                  0x4a, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01, // trace_id
                  0x52, 0x08, 0, 0, 0, 0, 0, 0, 0, 0x02, // span_id
                  0x59, 0x02, 0, 0, 0, 0, 0, 0, 0, // observed_time_unix_nano
        ];
        assert_eq!(bytes, expected);
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    #[test]
    fn a_line_inside_a_request_span_is_exported_with_its_span_and_trace_id() {
        let (address, receiving) = collector(&[200]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (own, stdout) = runtime.block_on(async {
            let export = LogExport::start(settings(address));
            let records = Arc::clone(&export.records);
            let logged = tokio::task::spawn_blocking(move || log_in_a_request(export))
                .await
                .unwrap();
            let deadline = tokio::time::Instant::now() + FINAL_EXPORT_WAIT;
            assert!(records.close().finished_by(deadline).await);
            logged
        });
        let [request] = receiving.join().unwrap().try_into().ok().unwrap();

        assert!(
            request.head.starts_with("post /otlp/v1/logs "),
            "{}",
            request.head
        );
        assert!(
            request
                .head
                .contains("content-type: application/x-protobuf"),
            "{}",
            request.head
        );
        assert!(stdout.contains("order placed"), "{stdout}");
        let body = &request.body;
        assert!(contains(body, b"order placed"));
        assert!(contains(body, b"attempt"));
        assert!(contains(body, &own.trace_id.to_be_bytes()));
        assert!(contains(body, &own.span_id.to_be_bytes()));
    }

    #[test]
    fn a_shutdown_on_a_current_thread_runtime_exports_the_last_lines() {
        let (address, receiving) = collector(&[200]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let out = Shared::default();
        runtime.block_on(async {
            let export = LogExport::start(settings(address));
            let records = Arc::clone(&export.records);
            let output = Output::Lines {
                filter: LogFilter::parse("info").unwrap(),
                sample: Sample::parse("1").unwrap(),
                writer: LogWriter::start(LineFormat::Json, out.clone(), Some(export)).unwrap(),
            };
            let dispatch = Dispatch::new(Telemetry::new(output, super::super::Exporter::silent()));
            let _default = tracing::dispatcher::set_default(&dispatch);
            tracing::info!("stopping for jane@example.com at 10.0.0.7");
            crate::flush_lines();
            let deadline = tokio::time::Instant::now() + FINAL_EXPORT_WAIT;
            assert!(records.close().finished_by(deadline).await);
        });
        let [request] = receiving.join().unwrap().try_into().ok().unwrap();

        assert!(contains(&request.body, b"stopping for *** at ***"));
    }

    #[test]
    fn a_stalled_export_does_not_hold_the_writer_s_flush_or_later_lines() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let stalled = LogExport::start(settings("127.0.0.1:9".parse().unwrap()));
        let out = Shared::default();
        let writer = LogWriter::start(LineFormat::Json, out.clone(), Some(stalled)).unwrap();
        let started = std::time::Instant::now();

        writer.send(crate::writer::tests::line(Level::INFO, "before"));
        writer.flush();
        writer.send(crate::writer::tests::line(Level::INFO, "after"));
        writer.flush();

        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert!(written(&out).contains("after"), "{}", written(&out));
    }

    fn log_in_a_request(export: LogExport) -> (TraceParent, String) {
        let out = Shared::default();
        let output = Output::Lines {
            filter: LogFilter::parse("info").unwrap(),
            sample: Sample::parse("1").unwrap(),
            writer: LogWriter::start(LineFormat::Json, out.clone(), Some(export)).unwrap(),
        };
        let dispatch = Dispatch::new(Telemetry::new(output, super::super::Exporter::silent()));
        let own = tracing::dispatcher::with_default(&dispatch, || {
            let request = tracing::info_span!("request", trace_id = Empty);
            let own = crate::continue_trace(&request, TraceParent::parse(INCOMING));
            request.in_scope(|| tracing::info!(attempt = 2_u64, "order placed"));
            own
        });
        dispatch.downcast_ref::<Telemetry>().unwrap().flush();
        (own, written(&out))
    }
}
