//! RESP, as Redis speaks it: <https://redis.io/docs/latest/develop/reference/protocol-spec/>.
//! Commands go out as RESP2 arrays; replies are read in RESP2 or RESP3.

use std::fmt::Write as _;

use bytes::{Buf, BufMut, BytesMut};
use sofie_core::{Error, Result};

/// Past it a reply is refused rather than buffered: a cached value is far smaller.
pub(crate) const MAX_REPLY_LENGTH: usize = 64 * 1024 * 1024;
/// Replies the cache reads nest at most twice; a deeper one, or a chain of attributes, could only
/// exhaust the stack.
const MAX_DEPTH: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Reply {
    Status(String),
    Error(String),
    Integer(i64),
    Bulk(Option<Vec<u8>>),
    Array(Option<Vec<Reply>>),
    /// RESP3's out-of-band data, such as a pub/sub message.
    Push(Vec<Reply>),
}

impl Reply {
    pub(crate) fn into_items(self) -> Option<Vec<Reply>> {
        match self {
            Self::Array(Some(items)) | Self::Push(items) => Some(items),
            _ => None,
        }
    }

    pub(crate) fn into_bytes(self) -> Option<Vec<u8>> {
        match self {
            Self::Bulk(Some(bytes)) => Some(bytes),
            Self::Status(text) => Some(text.into_bytes()),
            _ => None,
        }
    }
}

pub(crate) struct Command {
    arguments: Vec<Vec<u8>>,
}

impl Command {
    pub(crate) fn new(name: &str) -> Self {
        Self {
            arguments: vec![name.as_bytes().to_vec()],
        }
    }

    #[must_use]
    pub(crate) fn argument(mut self, argument: impl Into<Vec<u8>>) -> Self {
        self.arguments.push(argument.into());
        self
    }

    pub(crate) fn name(&self) -> String {
        String::from_utf8_lossy(&self.arguments[0]).into_owned()
    }

    pub(crate) fn encoded_length(&self) -> usize {
        self.arguments
            .iter()
            .map(|argument| argument.len() + 16)
            .sum::<usize>()
            + 16
    }

    pub(crate) fn encode(&self, out: &mut BytesMut) {
        let _ = write!(out, "*{}\r\n", self.arguments.len());
        for argument in &self.arguments {
            let _ = write!(out, "${}\r\n", argument.len());
            out.put_slice(argument);
            out.put_slice(b"\r\n");
        }
    }
}

/// Consumes one reply from the front of `buffer`, or nothing while it is incomplete.
pub(crate) fn parse(buffer: &mut BytesMut) -> Result<Option<Reply>> {
    let mut cursor = Cursor {
        bytes: buffer,
        at: 0,
        depth: 0,
    };
    let Some(reply) = cursor.reply()? else {
        return Ok(None);
    };
    let read = cursor.at;
    buffer.advance(read);
    Ok(Some(reply))
}

struct Cursor<'b> {
    bytes: &'b [u8],
    at: usize,
    depth: usize,
}

fn malformed(what: &str) -> Error {
    Error::internal_message(format!("Redis sent a malformed reply: {what}"))
}

impl Cursor<'_> {
    /// Up to a CRLF; a lone LF inside a line is part of it.
    fn line(&mut self) -> Option<&[u8]> {
        let rest = &self.bytes[self.at..];
        let mut from = 0;
        loop {
            let newline = from + rest[from..].iter().position(|byte| *byte == b'\n')?;
            if newline > 0 && rest[newline - 1] == b'\r' {
                self.at += newline + 1;
                return Some(&rest[..newline - 1]);
            }
            from = newline + 1;
        }
    }

    fn text_line(&mut self) -> Option<String> {
        let line = self.line()?;
        Some(String::from_utf8_lossy(line).into_owned())
    }

    fn number(&mut self) -> Result<Option<i64>> {
        let Some(line) = self.line() else {
            return Ok(None);
        };
        std::str::from_utf8(line)
            .ok()
            .and_then(|text| text.parse().ok())
            .map(Some)
            .ok_or_else(|| malformed("a length or integer is not a number"))
    }

    /// A bulk string; a negative length is RESP2's null.
    fn blob(&mut self) -> Result<Option<Reply>> {
        let Some(length) = self.number()? else {
            return Ok(None);
        };
        let Ok(length) = usize::try_from(length) else {
            return Ok(Some(Reply::Bulk(None)));
        };
        let end = self
            .at
            .checked_add(length)
            .ok_or_else(|| malformed("a bulk string longer than memory"))?;
        // Checked whole before copying, so a partial read copies nothing.
        match self.bytes.get(end..end + 2) {
            None => return Ok(None),
            Some(b"\r\n") => {}
            Some(_) => return Err(malformed("a bulk string does not end its line")),
        }
        let blob = self.bytes[self.at..end].to_vec();
        self.at = end + 2;
        Ok(Some(Reply::Bulk(Some(blob))))
    }

    /// An array, or a map as its keys and values in turn.
    fn items(&mut self, per_entry: usize) -> Result<Option<Reply>> {
        let Some(count) = self.number()? else {
            return Ok(None);
        };
        let Ok(count) = usize::try_from(count) else {
            return Ok(Some(Reply::Array(None)));
        };
        let total = count
            .checked_mul(per_entry)
            .ok_or_else(|| malformed("an aggregate longer than memory"))?;
        let mut items = Vec::with_capacity(total.min(1024));
        for _ in 0..total {
            let Some(item) = self.reply()? else {
                return Ok(None);
            };
            items.push(item);
        }
        Ok(Some(Reply::Array(Some(items))))
    }

    fn reply(&mut self) -> Result<Option<Reply>> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(malformed("replies nested too deep"));
        }
        let reply = self.one_reply();
        self.depth -= 1;
        reply
    }

    fn one_reply(&mut self) -> Result<Option<Reply>> {
        let Some(&kind) = self.bytes.get(self.at) else {
            return Ok(None);
        };
        self.at += 1;
        let reply = match kind {
            b'+' | b',' | b'(' => self.text_line().map(Reply::Status),
            b'-' => self.text_line().map(Reply::Error),
            b':' => self.number()?.map(Reply::Integer),
            b'_' => self.line().map(|_| Reply::Bulk(None)),
            b'#' => self
                .line()
                .map(|flag| Reply::Integer(i64::from(flag == b"t"))),
            b'$' => self.blob()?,
            b'!' => self.blob()?.map(|blob| {
                let message = blob.into_bytes().unwrap_or_default();
                Reply::Error(String::from_utf8_lossy(&message).into_owned())
            }),
            // Verbatim text starts with its format, such as `txt:`.
            b'=' => self.blob()?.map(|blob| {
                let text = blob.into_bytes().unwrap_or_default();
                Reply::Bulk(Some(text.get(4..).unwrap_or_default().to_vec()))
            }),
            b'*' | b'~' => self.items(1)?,
            b'%' => self.items(2)?,
            b'>' => self
                .items(1)?
                .map(|items| Reply::Push(items.into_items().unwrap_or_default())),
            // Attributes annotate the reply that follows, which is what is wanted.
            b'|' => match self.items(2)? {
                Some(_) => self.reply()?,
                None => None,
            },
            other => return Err(malformed(&format!("unknown type byte {other:#04x}"))),
        };
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(wire: &[u8]) -> Option<Reply> {
        let mut buffer = BytesMut::from(wire);
        let reply = parse(&mut buffer).unwrap();
        if reply.is_some() {
            assert!(buffer.is_empty(), "left unread: {buffer:?}");
        }
        reply
    }

    fn bulk(text: &str) -> Reply {
        Reply::Bulk(Some(text.as_bytes().to_vec()))
    }

    #[test]
    fn a_command_is_an_array_of_bulk_strings() {
        let mut out = BytesMut::new();
        Command::new("SET")
            .argument("k")
            .argument("a\r\nb")
            .encode(&mut out);

        assert_eq!(&out[..], b"*3\r\n$3\r\nSET\r\n$1\r\nk\r\n$4\r\na\r\nb\r\n");
    }

    #[test]
    fn resp2_replies_are_read() {
        assert_eq!(parsed(b"+OK\r\n"), Some(Reply::Status("OK".to_owned())));
        assert_eq!(
            parsed(b"-ERR no\r\n"),
            Some(Reply::Error("ERR no".to_owned()))
        );
        assert_eq!(parsed(b":-12\r\n"), Some(Reply::Integer(-12)));
        assert_eq!(parsed(b"$-1\r\n"), Some(Reply::Bulk(None)));
        assert_eq!(parsed(b"$4\r\na\r\nb\r\n"), Some(bulk("a\r\nb")));
        assert_eq!(parsed(b"*-1\r\n"), Some(Reply::Array(None)));
        assert_eq!(
            parsed(b"*2\r\n$1\r\nx\r\n*1\r\n:1\r\n"),
            Some(Reply::Array(Some(vec![
                bulk("x"),
                Reply::Array(Some(vec![Reply::Integer(1)]))
            ])))
        );
    }

    #[test]
    fn resp3_replies_are_read() {
        assert_eq!(parsed(b"_\r\n"), Some(Reply::Bulk(None)));
        assert_eq!(parsed(b"#t\r\n"), Some(Reply::Integer(1)));
        assert_eq!(parsed(b",1.5\r\n"), Some(Reply::Status("1.5".to_owned())));
        assert_eq!(
            parsed(b"!3\r\nERR\r\n"),
            Some(Reply::Error("ERR".to_owned()))
        );
        assert_eq!(parsed(b"=7\r\ntxt:abc\r\n"), Some(bulk("abc")));
        assert_eq!(
            parsed(b"%1\r\n+k\r\n:2\r\n"),
            Some(Reply::Array(Some(vec![
                Reply::Status("k".to_owned()),
                Reply::Integer(2)
            ])))
        );
        assert_eq!(
            parsed(b">2\r\n$7\r\nmessage\r\n$1\r\np\r\n"),
            Some(Reply::Push(vec![bulk("message"), bulk("p")]))
        );
        assert_eq!(
            parsed(b"|1\r\n+ttl\r\n:3\r\n:7\r\n"),
            Some(Reply::Integer(7))
        );
    }

    #[test]
    fn an_incomplete_reply_waits_for_more_and_consumes_nothing() {
        for wire in [&b"$5\r\nab"[..], b"*2\r\n:1\r\n", b"+O", b""] {
            let mut buffer = BytesMut::from(wire);
            assert_eq!(parse(&mut buffer).unwrap(), None);
            assert_eq!(buffer.len(), wire.len());
        }
    }

    #[test]
    fn two_replies_in_one_read_are_taken_one_at_a_time() {
        let mut buffer = BytesMut::from(&b"+A\r\n:2\r\n"[..]);

        assert_eq!(
            parse(&mut buffer).unwrap(),
            Some(Reply::Status("A".to_owned()))
        );
        assert_eq!(parse(&mut buffer).unwrap(), Some(Reply::Integer(2)));
        assert_eq!(parse(&mut buffer).unwrap(), None);
    }

    #[test]
    fn a_reply_nested_past_the_limit_is_refused() {
        let deep = "*1\r\n".repeat(MAX_DEPTH) + ":1\r\n";
        let attributes = "|0\r\n".repeat(MAX_DEPTH) + ":1\r\n";

        assert!(parse(&mut BytesMut::from(deep.as_bytes())).is_err());
        assert!(parse(&mut BytesMut::from(attributes.as_bytes())).is_err());
        assert!(parsed(b"*1\r\n*1\r\n:1\r\n").is_some());
    }

    #[test]
    fn a_line_ends_only_at_crlf() {
        assert_eq!(parsed(b"+a\nb\r\n"), Some(Reply::Status("a\nb".to_owned())));
    }

    #[test]
    fn an_unknown_type_is_refused() {
        assert!(parse(&mut BytesMut::from(&b"?x\r\n"[..])).is_err());
    }
}
