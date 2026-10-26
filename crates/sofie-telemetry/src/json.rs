use crate::fields::{FieldValue, Fields, push_formatted};
use crate::line::PendingLine;
use crate::timestamp::push_timestamp;

/// Recorded fields nest under `fields`, so one named `level` cannot overwrite the reserved key.
pub(crate) fn push_record(line: &mut String, record: &PendingLine) {
    line.push_str("{\"timestamp\":\"");
    push_timestamp(line, record.time);
    line.push_str("\",\"level\":\"");
    line.push_str(record.level.as_str());
    line.push('"');
    push_fields(line, &record.fields);
    line.push_str(",\"target\":");
    push_string(line, record.target);
    if !record.scope.is_empty() {
        line.push_str(",\"spans\":[");
        for (index, (name, fields)) in record.scope.iter().enumerate() {
            if index > 0 {
                line.push(',');
            }
            line.push_str("{\"name\":");
            push_string(line, name);
            push_fields(line, fields.scrubbed());
            line.push('}');
        }
        line.push(']');
    }
    line.push('}');
}

fn push_fields(line: &mut String, fields: &Fields) {
    if fields.is_empty() {
        return;
    }
    line.push_str(",\"fields\":{");
    for (index, (name, value)) in fields.iter().enumerate() {
        if index > 0 {
            line.push(',');
        }
        push_string(line, name);
        line.push(':');
        push_value(line, value);
    }
    line.push('}');
}

/// A float JSON cannot hold, NaN or an infinity, is `null`.
fn push_value(line: &mut String, value: &FieldValue) {
    match value {
        FieldValue::Text(text) | FieldValue::Formatted(text) | FieldValue::Composed(text) => {
            push_string(line, text);
        }
        FieldValue::Signed(number) => push_formatted(line, format_args!("{number}")),
        FieldValue::Unsigned(number) => push_formatted(line, format_args!("{number}")),
        FieldValue::Float(number) if number.is_finite() => {
            push_formatted(line, format_args!("{number:?}"));
        }
        FieldValue::Float(_) => line.push_str("null"),
        FieldValue::Bool(value) => line.push_str(if *value { "true" } else { "false" }),
    }
}

/// Every byte JSON must escape is ASCII, so text is copied in runs between them.
fn push_string(line: &mut String, text: &str) {
    line.push('"');
    let mut copied = 0;
    for (index, byte) in text.bytes().enumerate() {
        if byte >= b' ' && byte != b'"' && byte != b'\\' {
            continue;
        }
        line.push_str(&text[copied..index]);
        push_escape(line, byte);
        copied = index + 1;
    }
    line.push_str(&text[copied..]);
    line.push('"');
}

fn push_escape(line: &mut String, byte: u8) {
    match byte {
        b'"' => line.push_str("\\\""),
        b'\\' => line.push_str("\\\\"),
        b'\n' => line.push_str("\\n"),
        b'\r' => line.push_str("\\r"),
        b'\t' => line.push_str("\\t"),
        control => push_formatted(line, format_args!("\\u{control:04x}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{push_string, push_value};
    use crate::fields::FieldValue;

    fn string(text: &str) -> String {
        let mut line = String::new();
        push_string(&mut line, text);
        line
    }

    fn value(value: &FieldValue) -> String {
        let mut line = String::new();
        push_value(&mut line, value);
        line
    }

    #[test]
    fn strings_escape_quotes_backslashes_and_every_control_character() {
        assert_eq!(string(r#"say "hi" \ bye"#), r#""say \"hi\" \\ bye""#);
        assert_eq!(string("a\nb\rc\td"), r#""a\nb\rc\td""#);
        assert_eq!(
            string("\u{0}\u{8}\u{c}\u{1b}\u{1f}"),
            r#""\u0000\u0008\u000c\u001b\u001f""#
        );
    }

    #[test]
    fn non_ascii_text_is_kept_as_it_is() {
        assert_eq!(
            string("Grüße, 東京 🚀\u{2028}"),
            "\"Grüße, 東京 🚀\u{2028}\""
        );
        assert_eq!(string("é\"ü"), "\"é\\\"ü\"");
    }

    #[test]
    fn numbers_and_booleans_are_json_values() {
        assert_eq!(value(&FieldValue::Signed(i64::MIN)), "-9223372036854775808");
        assert_eq!(
            value(&FieldValue::Unsigned(u64::MAX)),
            "18446744073709551615"
        );
        assert_eq!(value(&FieldValue::Float(0.31)), "0.31");
        assert_eq!(value(&FieldValue::Float(1.0)), "1.0");
        assert_eq!(value(&FieldValue::Float(1e300)), "1e300");
        assert_eq!(value(&FieldValue::Float(f64::NAN)), "null");
        assert_eq!(value(&FieldValue::Float(f64::NEG_INFINITY)), "null");
        assert_eq!(value(&FieldValue::Bool(false)), "false");
        assert_eq!(
            value(&FieldValue::Formatted("Some(1)".to_owned())),
            "\"Some(1)\""
        );
    }
}
