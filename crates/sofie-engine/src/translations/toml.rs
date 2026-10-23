//! Anything beyond tables, dotted keys and double-quoted strings is refused, so a file never
//! means something other than what a TOML reader makes of it.

#[derive(Debug)]
pub(super) struct Entry {
    pub key: String,
    pub text: String,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug)]
pub(super) struct Syntax {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

pub(super) fn parse(source: &str) -> Result<Vec<Entry>, Syntax> {
    let mut table = String::new();
    let mut entries = Vec::new();
    for (index, text) in source.lines().enumerate() {
        let mut reader = LineReader {
            text,
            offset: 0,
            line: index + 1,
        };
        reader.skip_spaces();
        if reader.is_done() {
            continue;
        }
        if reader.eat('[') {
            table = reader.table_name()?;
            continue;
        }
        let column = reader.column();
        let name = reader.key()?;
        reader.expect('=', "after the key")?;
        let translation = reader.string()?;
        reader.finish()?;
        let key = if table.is_empty() {
            name
        } else {
            format!("{table}.{name}")
        };
        entries.push(Entry {
            key,
            text: translation,
            line: index + 1,
            column,
        });
    }
    refuse_clashes(&mut entries)?;
    Ok(entries)
}

/// TOML refuses a key given twice, and a key that is both a text and a
/// table of further keys.
fn refuse_clashes(entries: &mut [Entry]) -> Result<(), Syntax> {
    entries.sort_by(|left, right| left.key.cmp(&right.key).then(left.line.cmp(&right.line)));
    for pair in entries.windows(2) {
        let [first, second] = pair else { continue };
        let message = if first.key == second.key {
            format!(
                "`{}` is defined twice, first on line {}",
                second.key, first.line
            )
        } else if second
            .key
            .strip_prefix(first.key.as_str())
            .is_some_and(|rest| rest.starts_with('.'))
        {
            format!(
                "`{}` is a text on line {}, so it cannot also hold `{}`",
                first.key, first.line, second.key
            )
        } else {
            continue;
        };
        return Err(Syntax {
            line: second.line,
            column: second.column,
            message,
        });
    }
    Ok(())
}

struct LineReader<'t> {
    text: &'t str,
    offset: usize,
    line: usize,
}

impl LineReader<'_> {
    fn peek(&self) -> Option<char> {
        self.text.get(self.offset..)?.chars().next()
    }

    fn advance(&mut self) -> Option<char> {
        let next = self.peek()?;
        self.offset += next.len_utf8();
        Some(next)
    }

    fn eat(&mut self, expected: char) -> bool {
        let is_next = self.peek() == Some(expected);
        if is_next {
            self.offset += expected.len_utf8();
        }
        is_next
    }

    fn skip_spaces(&mut self) {
        while self.peek().is_some_and(|next| next == ' ' || next == '\t') {
            self.advance();
        }
    }

    fn is_done(&self) -> bool {
        self.peek().is_none_or(|next| next == '#')
    }

    fn column(&self) -> usize {
        self.text
            .get(..self.offset)
            .map_or(0, |before| before.chars().count())
            + 1
    }

    fn error(&self, message: impl Into<String>) -> Syntax {
        Syntax {
            line: self.line,
            column: self.column(),
            message: message.into(),
        }
    }

    fn expect(&mut self, expected: char, context: &str) -> Result<(), Syntax> {
        self.skip_spaces();
        if self.eat(expected) {
            self.skip_spaces();
            return Ok(());
        }
        Err(self.error(format!("expected `{expected}` {context}")))
    }

    fn table_name(&mut self) -> Result<String, Syntax> {
        self.skip_spaces();
        let name = self.key()?;
        self.expect(']', "to end the table name")?;
        self.finish()?;
        Ok(name)
    }

    fn key(&mut self) -> Result<String, Syntax> {
        let mut key = String::new();
        loop {
            let start = self.offset;
            while self.peek().is_some_and(is_key_character) {
                self.advance();
            }
            let segment = self.text.get(start..self.offset).unwrap_or_default();
            if segment.is_empty() {
                return Err(self.error(
                    "expected a key of lowercase letters, digits and `_`, such as `title`",
                ));
            }
            key.push_str(segment);
            self.skip_spaces();
            if !self.eat('.') {
                return Ok(key);
            }
            self.skip_spaces();
            key.push('.');
        }
    }

    fn string(&mut self) -> Result<String, Syntax> {
        if !self.eat('"') {
            return Err(self.error("a translation is text in double quotes, such as \"Orders\""));
        }
        let mut text = String::new();
        loop {
            match self.advance() {
                None => return Err(self.error("the text is never closed with `\"`")),
                Some('"') => return Ok(text),
                Some('\\') => text.push(self.escape()?),
                Some(character) => text.push(character),
            }
        }
    }

    fn escape(&mut self) -> Result<char, Syntax> {
        match self.advance() {
            Some('"') => Ok('"'),
            Some('\\') => Ok('\\'),
            Some('n') => Ok('\n'),
            Some('t') => Ok('\t'),
            Some('u') => self.unicode_escape(),
            _ => {
                Err(self.error("unsupported escape; use `\\\"`, `\\\\`, `\\n`, `\\t` or `\\uXXXX`"))
            }
        }
    }

    fn unicode_escape(&mut self) -> Result<char, Syntax> {
        let digits = self
            .text
            .get(self.offset..self.offset + 4)
            .unwrap_or_default();
        let character = u32::from_str_radix(digits, 16)
            .ok()
            .filter(|_| digits.len() == 4)
            .and_then(char::from_u32)
            .ok_or_else(|| self.error("`\\u` needs four hexadecimal digits of a character"))?;
        self.offset += 4;
        Ok(character)
    }

    fn finish(&mut self) -> Result<(), Syntax> {
        self.skip_spaces();
        if self.is_done() {
            return Ok(());
        }
        Err(self.error(format!(
            "unexpected `{}`; a line holds one key and its text",
            self.peek().unwrap_or_default()
        )))
    }
}

pub(super) fn is_key_character(character: char) -> bool {
    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
}

#[cfg(test)]
mod tests {
    use super::{Syntax, parse};

    fn entries(source: &str) -> Vec<(String, String)> {
        parse(source)
            .unwrap_or_else(|error| panic!("{error:?}"))
            .into_iter()
            .map(|entry| (entry.key, entry.text))
            .collect()
    }

    fn error(source: &str) -> (usize, usize, String) {
        let Syntax {
            line,
            column,
            message,
        } = parse(source).expect_err("the source is refused");
        (line, column, message)
    }

    #[test]
    fn reads_tables_dotted_keys_escapes_and_comments() {
        let source = "# Orders\ntitle = \"Orders\" # trailing\n\n[label]\ntotal=\"Sum \\\"all\\\"\"\n[enum . order_status]\npaid = \"Pa\\u00efd\\n\"\na . b = \"x\"\n";
        assert_eq!(
            entries(source),
            [
                ("enum.order_status.a.b".to_owned(), "x".to_owned()),
                (
                    "enum.order_status.paid".to_owned(),
                    "Pa\u{ef}d\n".to_owned()
                ),
                ("label.total".to_owned(), "Sum \"all\"".to_owned()),
                ("title".to_owned(), "Orders".to_owned()),
            ]
        );
    }

    #[test]
    fn refuses_what_is_not_the_subset_at_its_place() {
        assert_eq!(error("a = 1").0, 1);
        assert_eq!(error("\nTitle = \"x\"").1, 1);
        assert!(error("a = \"x").2.contains("never closed"));
        assert!(error("a = \"\\q\"").2.contains("unsupported escape"));
        assert!(error("a = \"x\" b").2.contains("unexpected `b`"));
        assert!(error("[a\nb = \"x\"").2.contains("`]`"));
        assert!(error("a = \"\\u00\"").2.contains("four"));
        assert!(error("\"a\" = \"x\"").2.contains("lowercase"));
    }

    #[test]
    fn refuses_a_key_twice_or_as_text_and_table() {
        assert_eq!(
            error("a = \"x\"\n[b]\nc = \"y\"\n[b]\nc = \"z\""),
            (5, 1, "`b.c` is defined twice, first on line 3".to_owned())
        );
        assert!(
            error("a = \"x\"\na.b = \"y\"")
                .2
                .contains("cannot also hold")
        );
    }
}
