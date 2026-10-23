//! Closed spans, as `ExportTraceServiceRequest`s.

use std::borrow::Cow;
use std::time::{Duration, SystemTime};

use super::ClosedSpan;
use super::batch::{Queue, Signal};
use super::common::{self, Attribute, Value};
use super::protobuf::Encoder;
use crate::TraceParent;
use crate::outage::Output;
use crate::scrub;

const SCOPE: &str = "sofie";

mod request {
    pub(super) const RESOURCE_SPANS: u32 = 1;
    pub(super) const SCOPE_SPANS: u32 = 2;
    pub(super) const SPANS: u32 = 2;
}

mod span {
    pub(super) const TRACE_ID: u32 = 1;
    pub(super) const SPAN_ID: u32 = 2;
    pub(super) const PARENT_SPAN_ID: u32 = 4;
    pub(super) const NAME: u32 = 5;
    pub(super) const KIND: u32 = 6;
    pub(super) const START_TIME_UNIX_NANO: u32 = 7;
    pub(super) const END_TIME_UNIX_NANO: u32 = 8;
    pub(super) const ATTRIBUTES: u32 = 9;
    pub(super) const STATUS: u32 = 15;
    pub(super) const FLAGS: u32 = 16;
    /// `SpanFlags`: whether the parent is remote is known, and whether it is.
    pub(super) const HAS_IS_REMOTE: u32 = 0x100;
    pub(super) const IS_REMOTE: u32 = 0x200;
}

mod status {
    pub(super) const MESSAGE: u32 = 2;
    pub(super) const CODE: u32 = 3;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ParentSpan {
    None,
    Local(u64),
    Remote(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpanKind {
    Internal = 1,
    Server = 2,
    Client = 3,
    Producer = 4,
    Consumer = 5,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Unset,
    Ok,
    Error(String),
}

impl Status {
    fn code(&self) -> u64 {
        match self {
            Self::Unset => 0,
            Self::Ok => 1,
            Self::Error(_) => 2,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SpanRecord {
    pub(crate) own: TraceParent,
    pub(crate) parent: ParentSpan,
    pub(crate) name: Cow<'static, str>,
    pub(crate) kind: SpanKind,
    pub(crate) start: SystemTime,
    pub(crate) end: SystemTime,
    pub(crate) attributes: Vec<Attribute>,
    pub(crate) status: Status,
}

pub(crate) struct Spans;

impl Signal for Spans {
    type Item = ClosedSpan;
    const PATH: &'static str = "/v1/traces";
    const OUTPUT: Output = Output::Spans;
    /// The SDK batch span processor's defaults.
    const CAPACITY: usize = 2048;
    const BATCH_SIZE: usize = 512;
    const INTERVAL: Duration = Duration::from_secs(5);

    fn encode(&self, closed: Vec<ClosedSpan>, resource: &[u8]) -> Vec<u8> {
        let mut spans: Vec<SpanRecord> = closed.into_iter().map(ClosedSpan::into_record).collect();
        spans.iter_mut().for_each(scrub_span);
        encode(&spans, resource)
    }

    fn report_dropped(queue: &Queue<ClosedSpan>) {
        let dropped = queue.take_dropped();
        if dropped > 0 {
            tracing::warn!(dropped, "spans dropped: the OTLP export fell behind");
        }
    }
}

/// A log line's rules, applied on the exporter's task rather than where the span closed.
pub(crate) fn scrub_span(span: &mut SpanRecord) {
    if let Some(clean) = scrub::scrubbed_text(&span.name) {
        span.name = Cow::Owned(clean);
    }
    if let Status::Error(description) = &mut span.status
        && let Some(clean) = scrub::scrubbed_text(description)
    {
        *description = clean;
    }
    for attribute in &mut span.attributes {
        if scrub::is_sensitive_key(attribute.key) {
            attribute.value = Value::from(scrub::MASK);
        } else if let Value::Text(text) = &attribute.value
            && let Some(clean) = scrub::scrubbed_text(text)
        {
            attribute.value = Value::Text(Cow::Owned(clean));
        }
    }
}

pub(crate) fn encode(spans: &[SpanRecord], resource: &[u8]) -> Vec<u8> {
    let mut encoder = Encoder::default();
    encoder.message(request::RESOURCE_SPANS, |encoder| {
        common::resource(encoder, resource);
        encoder.message(request::SCOPE_SPANS, |encoder| {
            common::scope(encoder, SCOPE);
            for record in spans {
                encoder.message(request::SPANS, |encoder| encode_span(encoder, record));
            }
        });
    });
    encoder.into_bytes()
}

fn encode_span(encoder: &mut Encoder, record: &SpanRecord) {
    encoder.bytes(span::TRACE_ID, &record.own.trace_id.to_be_bytes());
    encoder.bytes(span::SPAN_ID, &record.own.span_id.to_be_bytes());
    let mut flags = u32::from(record.own.flags) | span::HAS_IS_REMOTE;
    match record.parent {
        ParentSpan::None => {}
        ParentSpan::Local(parent) => encoder.bytes(span::PARENT_SPAN_ID, &parent.to_be_bytes()),
        ParentSpan::Remote(parent) => {
            encoder.bytes(span::PARENT_SPAN_ID, &parent.to_be_bytes());
            flags |= span::IS_REMOTE;
        }
    }
    encoder.string(span::NAME, &record.name);
    encoder.varint(span::KIND, record.kind as u64);
    encoder.fixed64(span::START_TIME_UNIX_NANO, common::unix_nanos(record.start));
    encoder.fixed64(span::END_TIME_UNIX_NANO, common::unix_nanos(record.end));
    common::attributes(encoder, span::ATTRIBUTES, &record.attributes);
    if record.status != Status::Unset {
        encoder.message(span::STATUS, |encoder| {
            if let Status::Error(description) = &record.status
                && !description.is_empty()
            {
                encoder.string(status::MESSAGE, description);
            }
            encoder.varint(status::CODE, record.status.code());
        });
    }
    encoder.fixed32(span::FLAGS, flags);
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    #[test]
    fn a_span_encodes_to_the_bytes_the_proto_definitions_give() {
        let record = SpanRecord {
            own: TraceParent {
                trace_id: 0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10,
                span_id: 0x1112_1314_1516_1718,
                flags: 1,
            },
            parent: ParentSpan::Remote(0x2122_2324_2526_2728),
            name: Cow::Borrowed("GET"),
            kind: SpanKind::Server,
            start: UNIX_EPOCH + Duration::from_nanos(1),
            end: UNIX_EPOCH + Duration::from_nanos(2),
            attributes: vec![Attribute::new("n", 7)],
            status: Status::Error("x".to_owned()),
        };
        let resource = [0x0a, 0x00];

        let bytes = encode(&[record], &resource);

        #[rustfmt::skip]
        let expected: &[u8] = &[
            0x0a, 0x66, // resource_spans, 102 bytes
              0x0a, 0x02, 0x0a, 0x00, // resource: the given body
              0x12, 0x60, // scope_spans, 96 bytes
                0x0a, 0x07, 0x0a, 0x05, b's', b'o', b'f', b'i', b'e', // scope { name }
                0x12, 0x55, // spans, 85 bytes
                  0x0a, 0x10, // trace_id, 16 bytes
                    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
                    0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
                  0x12, 0x08, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, // span_id
                  0x22, 0x08, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, // parent_span_id
                  0x2a, 0x03, b'G', b'E', b'T', // name
                  0x30, 0x02, // kind: server
                  0x39, 0x01, 0, 0, 0, 0, 0, 0, 0, // start_time_unix_nano
                  0x41, 0x02, 0, 0, 0, 0, 0, 0, 0, // end_time_unix_nano
                  0x4a, 0x07, 0x0a, 0x01, b'n', 0x12, 0x02, 0x18, 0x07, // attributes { n: int 7 }
                  0x7a, 0x05, 0x12, 0x01, b'x', 0x18, 0x02, // status { message, code: error }
                  0x85, 0x01, 0x01, 0x03, 0x00, 0x00, // flags: sampled, parent known remote
        ];
        assert_eq!(bytes, expected);
    }
}
