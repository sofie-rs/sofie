//! Two-space indentation and keys in the order given, so the output is stable in a diff.

use std::fmt::Write as _;

pub(crate) enum Json {
    Object(Vec<(String, Json)>),
    Array(Vec<Json>),
    Text(String),
    Boolean(bool),
}

impl Json {
    pub(crate) fn object<const FIELDS: usize>(fields: [(&str, Json); FIELDS]) -> Self {
        Self::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
        )
    }

    pub(crate) fn text(text: &str) -> Self {
        Self::Text(text.to_owned())
    }

    pub(crate) fn render(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out.push('\n');
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        match self {
            Self::Object(fields) if fields.is_empty() => out.push_str("{}"),
            Self::Array(items) if items.is_empty() => out.push_str("[]"),
            Self::Object(fields) => {
                out.push('{');
                for (index, (key, value)) in fields.iter().enumerate() {
                    separate(out, index, depth + 1);
                    write_string(out, key);
                    out.push_str(": ");
                    value.write(out, depth + 1);
                }
                close(out, depth, '}');
            }
            Self::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    separate(out, index, depth + 1);
                    item.write(out, depth + 1);
                }
                close(out, depth, ']');
            }
            Self::Text(text) => write_string(out, text),
            Self::Boolean(value) => out.push_str(if *value { "true" } else { "false" }),
        }
    }
}

fn separate(out: &mut String, index: usize, depth: usize) {
    if index > 0 {
        out.push(',');
    }
    out.push('\n');
    out.push_str(&"  ".repeat(depth));
}

fn close(out: &mut String, depth: usize, bracket: char) {
    out.push('\n');
    out.push_str(&"  ".repeat(depth));
    out.push(bracket);
}

fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            control if control < ' ' => {
                let _ = write!(out, "\\u{:04x}", u32::from(control));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::Json;

    #[test]
    fn writes_indented_json_with_escaped_strings() {
        let document = Json::object([
            ("a", Json::text("say \"hi\"\n")),
            (
                "b",
                Json::Array(vec![Json::Boolean(true), Json::Object(Vec::new())]),
            ),
        ]);

        assert_eq!(
            document.render(),
            "{\n  \"a\": \"say \\\"hi\\\"\\n\",\n  \"b\": [\n    true,\n    {}\n  ]\n}\n"
        );
    }
}
