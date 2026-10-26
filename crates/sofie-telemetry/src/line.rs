use std::sync::Arc;
use std::time::SystemTime;

use tracing::Level;

use crate::TraceParent;
use crate::fields::{FieldValue, Fields, push_formatted};
use crate::json;
use crate::scrub::SpanFields;
use crate::timestamp::push_timestamp;

const MESSAGE: &str = "message";

#[derive(Clone, Copy, Debug)]
pub(crate) enum LineFormat {
    Json,
    Readable,
}

/// Owned so the writer's thread can format it.
pub(crate) struct PendingLine {
    pub(crate) level: Level,
    pub(crate) target: &'static str,
    pub(crate) fields: Fields,
    pub(crate) scope: Vec<(&'static str, Arc<SpanFields>)>,
    #[cfg_attr(
        not(feature = "otlp"),
        expect(dead_code, reason = "only the OTLP export links a line to its span")
    )]
    pub(crate) trace: Option<TraceParent>,
    pub(crate) time: SystemTime,
}

impl LineFormat {
    pub(crate) fn of_build() -> Self {
        if cfg!(debug_assertions) {
            return Self::Readable;
        }
        Self::Json
    }

    pub(crate) fn render_into(self, line: &mut String, record: &PendingLine) {
        match self {
            Self::Json => json::push_record(line, record),
            Self::Readable => push_readable(line, record),
        }
        line.push('\n');
    }
}

fn push_readable(line: &mut String, record: &PendingLine) {
    push_timestamp(line, record.time);
    push_formatted(line, format_args!(" {:>5} ", record.level.as_str()));
    for (name, fields) in &record.scope {
        line.push_str(name);
        if !fields.is_empty() {
            line.push('{');
            push_readable_fields(line, fields.scrubbed());
            line.push('}');
        }
        line.push(':');
    }
    if !record.scope.is_empty() {
        line.push(' ');
    }
    line.push_str(record.target);
    line.push_str(": ");
    push_readable_fields(line, &record.fields);
}

fn push_readable_fields(line: &mut String, fields: &Fields) {
    for (index, (name, value)) in fields.iter().enumerate() {
        if index > 0 {
            line.push(' ');
        }
        if name == MESSAGE {
            push_readable_value(line, value);
            continue;
        }
        line.push_str(name);
        line.push('=');
        push_readable_value(line, value);
    }
}

pub(crate) fn push_readable_value(line: &mut String, value: &FieldValue) {
    match value {
        FieldValue::Text(text) => push_formatted(line, format_args!("{text:?}")),
        FieldValue::Formatted(text) => push_without_controls(line, text),
        FieldValue::Composed(text) => line.push_str(text),
        other => push_formatted(line, format_args!("{other}")),
    }
}

/// A logged error may carry a caller's text; a newline or terminal escape in it must not
/// forge another line or recolour the terminal.
fn push_without_controls(line: &mut String, text: &str) {
    if !text.contains(char::is_control) {
        line.push_str(text);
        return;
    }
    for character in text.chars() {
        if character.is_control() {
            push_formatted(line, format_args!("{}", character.escape_debug()));
        } else {
            line.push(character);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use tracing::Level;

    use super::{LineFormat, PendingLine};
    use crate::fields::{FieldValue, Fields};
    use crate::scrub::SpanFields;

    fn noon() -> SystemTime {
        UNIX_EPOCH + Duration::from_hours(497_700)
    }

    fn fields(values: &[(&'static str, FieldValue)]) -> Fields {
        let mut fields = Fields::default();
        for (name, value) in values {
            fields.set(name, value.clone());
        }
        fields
    }

    fn render(format: LineFormat, scope: Vec<(&'static str, Arc<SpanFields>)>) -> String {
        let event = fields(&[
            (
                "message",
                FieldValue::Formatted("request finished".to_owned()),
            ),
            ("status", FieldValue::Unsigned(200)),
            ("path", FieldValue::Text("/a\"b\n".to_owned())),
        ]);
        let record = PendingLine {
            level: Level::INFO,
            target: "sofie_http::trace",
            fields: event,
            scope,
            trace: None,
            time: noon(),
        };
        let mut line = String::new();
        format.render_into(&mut line, &record);
        line
    }

    #[test]
    fn a_readable_line_shows_the_spans_then_the_event() {
        let request = fields(&[("otel.name", FieldValue::Text("GET /".to_owned()))]);
        let scope = vec![
            ("request", Arc::new(SpanFields::new(request))),
            ("handler", Arc::new(SpanFields::default())),
        ];

        assert_eq!(
            render(LineFormat::Readable, scope),
            "2026-10-11T12:00:00.000000Z  INFO request{otel.name=\"GET /\"}:handler: \
             sofie_http::trace: request finished status=200 path=\"/a\\\"b\\n\"\n"
        );
    }

    #[test]
    fn a_readable_line_outside_any_span_starts_with_the_target() {
        assert_eq!(
            render(LineFormat::Readable, Vec::new()),
            "2026-10-11T12:00:00.000000Z  INFO sofie_http::trace: request finished status=200 \
             path=\"/a\\\"b\\n\"\n"
        );
    }

    #[test]
    fn a_formatted_value_cannot_forge_a_line_or_a_terminal_escape() {
        let record = PendingLine {
            level: Level::WARN,
            target: "app",
            fields: fields(&[
                (
                    "message",
                    FieldValue::Formatted("failed\n0000 ERROR forged".to_owned()),
                ),
                ("error", FieldValue::Formatted("\u{1b}[31mred".to_owned())),
            ]),
            scope: Vec::new(),
            trace: None,
            time: noon(),
        };
        let mut line = String::new();

        LineFormat::Readable.render_into(&mut line, &record);

        assert_eq!(line.matches('\n').count(), 1, "{line}");
        assert!(
            line.ends_with("failed\\n0000 ERROR forged error=\\u{1b}[31mred\n"),
            "{line}"
        );
    }

    #[test]
    fn a_json_line_carries_the_event_fields_and_each_span() {
        let request = fields(&[
            ("trace_id", FieldValue::Text("4bf9".to_owned())),
            ("attempt", FieldValue::Signed(-1)),
        ]);
        assert_eq!(
            render(
                LineFormat::Json,
                vec![("request", Arc::new(SpanFields::new(request)))]
            ),
            "{\"timestamp\":\"2026-10-11T12:00:00.000000Z\",\"level\":\"INFO\",\
             \"fields\":{\"message\":\"request finished\",\"status\":200,\"path\":\"/a\\\"b\\n\"},\
             \"target\":\"sofie_http::trace\",\
             \"spans\":[{\"name\":\"request\",\"fields\":{\"trace_id\":\"4bf9\",\"attempt\":-1}}]}\n"
        );
    }

    #[test]
    fn a_json_field_named_like_a_reserved_key_cannot_replace_it() {
        let record = PendingLine {
            level: Level::INFO,
            target: "app",
            fields: fields(&[
                ("level", FieldValue::Text("ERROR".to_owned())),
                ("timestamp", FieldValue::Text("forged".to_owned())),
            ]),
            scope: vec![(
                "request",
                Arc::new(SpanFields::new(fields(&[(
                    "name",
                    FieldValue::Text("forged".to_owned()),
                )]))),
            )],
            trace: None,
            time: noon(),
        };
        let mut line = String::new();

        LineFormat::Json.render_into(&mut line, &record);

        assert_eq!(
            line,
            "{\"timestamp\":\"2026-10-11T12:00:00.000000Z\",\"level\":\"INFO\",\
             \"fields\":{\"level\":\"ERROR\",\"timestamp\":\"forged\"},\"target\":\"app\",\
             \"spans\":[{\"name\":\"request\",\"fields\":{\"name\":\"forged\"}}]}\n"
        );
    }

    #[test]
    fn a_json_line_outside_any_span_has_no_span_list() {
        let line = render(LineFormat::Json, Vec::new());

        assert!(
            line.ends_with("\"target\":\"sofie_http::trace\"}\n"),
            "{line}"
        );
    }
}
