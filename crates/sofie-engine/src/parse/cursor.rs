use crate::syntax::{LocatedError, Position};

/// Every syntax character is ASCII, so the cursor only stops on character boundaries.
#[derive(Clone)]
pub(crate) struct Cursor<'s> {
    source: &'s str,
    offset: usize,
    line: usize,
    column: usize,
}

impl<'s> Cursor<'s> {
    pub(crate) fn new(source: &'s str) -> Self {
        Self {
            source,
            offset: 0,
            line: 1,
            column: 1,
        }
    }

    pub(crate) fn position(&self) -> Position {
        Position {
            line: self.line,
            column: self.column,
        }
    }

    pub(crate) fn offset(&self) -> usize {
        self.offset
    }

    pub(crate) fn is_at_end(&self) -> bool {
        self.offset >= self.source.len()
    }

    pub(crate) fn peek(&self) -> Option<u8> {
        self.source.as_bytes().get(self.offset).copied()
    }

    pub(crate) fn starts_with(&self, text: &str) -> bool {
        self.rest().starts_with(text)
    }

    pub(crate) fn advance(&mut self) {
        let Some(byte) = self.peek() else {
            return;
        };
        self.offset += 1;
        if byte == b'\n' {
            self.line += 1;
            self.column = 1;
        } else if !is_continuation_byte(byte) {
            self.column += 1;
        }
    }

    pub(crate) fn eat(&mut self, text: &str) -> bool {
        if !self.starts_with(text) {
            return false;
        }
        for _ in 0..text.len() {
            self.advance();
        }
        true
    }

    pub(crate) fn expect(&mut self, text: &str, context: &str) -> Result<(), LocatedError> {
        if self.eat(text) {
            return Ok(());
        }
        Err(self.unexpected(format_args!("expected `{text}` {context}")))
    }

    pub(crate) fn skip_whitespace(&mut self) {
        self.take_while(|byte| byte.is_ascii_whitespace());
    }

    pub(crate) fn take_while(&mut self, accepts: impl Fn(u8) -> bool) -> &'s str {
        let start = self.offset;
        while self.peek().is_some_and(&accepts) {
            self.advance();
        }
        self.slice(start, self.offset)
    }

    pub(crate) fn take_identifier(&mut self) -> &'s str {
        if !self.peek().is_some_and(is_identifier_start) {
            return "";
        }
        self.take_while(is_identifier_byte)
    }

    pub(crate) fn slice(&self, start: usize, end: usize) -> &'s str {
        self.source.get(start..end).unwrap_or_default()
    }

    pub(crate) fn error(&self, message: impl Into<String>) -> LocatedError {
        LocatedError::new(self.position(), message)
    }

    pub(crate) fn unexpected(&self, expected: impl std::fmt::Display) -> LocatedError {
        self.error(format!("{expected}, found {}", self.describe_next()))
    }

    pub(crate) fn describe_next(&self) -> String {
        self.rest().chars().next().map_or_else(
            || "the end of the template".to_owned(),
            |next| format!("`{next}`"),
        )
    }

    fn rest(&self) -> &'s str {
        self.source.get(self.offset..).unwrap_or_default()
    }
}

fn is_continuation_byte(byte: u8) -> bool {
    byte & 0xC0 == 0x80
}

pub(super) fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

pub(super) fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::Cursor;

    #[test]
    fn tracks_lines_and_character_columns() {
        let mut cursor = Cursor::new("é\nab");
        cursor.take_while(|byte| byte != b'\n');
        assert_eq!(cursor.position().column, 2);
        cursor.advance();
        cursor.advance();
        assert_eq!((cursor.position().line, cursor.position().column), (2, 2));
    }

    #[test]
    fn eat_consumes_only_matching_text() {
        let mut cursor = Cursor::new("{#if");
        assert!(!cursor.eat("{/"));
        assert!(cursor.eat("{#"));
        assert_eq!(cursor.take_identifier(), "if");
        assert!(cursor.is_at_end());
    }

    #[test]
    fn describes_the_end_of_input() {
        assert_eq!(Cursor::new("").describe_next(), "the end of the template");
    }
}
