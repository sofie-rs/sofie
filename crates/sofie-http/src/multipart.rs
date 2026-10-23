//! `multipart/form-data` (RFC 7578) read as it arrives, so a file never sits whole in memory.

use bytes::{Buf, Bytes, BytesMut};
use http::HeaderMap;
use http::header::CONTENT_TYPE;
use http_body_util::BodyExt;
use hyper::body::Body;

const MEDIA_TYPE: &str = "multipart/form-data";
const HEAD_LIMIT: usize = 8 * 1024;
const BOUNDARY_LIMIT: usize = 70;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Stop {
    TooLarge,
    Malformed(&'static str),
    Unread(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PartHead {
    pub(crate) name: String,
    /// Present, perhaps empty, for a file input.
    pub(crate) filename: Option<String>,
    pub(crate) content_type: Option<String>,
}

pub(crate) fn boundary(headers: &HeaderMap) -> Option<Result<String, Stop>> {
    let value = headers.get(CONTENT_TYPE)?.to_str().ok()?;
    let (media_type, parameters) = value.split_once(';').unwrap_or((value, ""));
    if !media_type.trim().eq_ignore_ascii_case(MEDIA_TYPE) {
        return None;
    }
    let boundary = parameters_of(parameters)
        .into_iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("boundary"))
        .map(|(_, value)| value)
        .filter(|boundary| (1..=BOUNDARY_LIMIT).contains(&boundary.len()));
    Some(boundary.ok_or(Stop::Malformed(
        "it names no boundary of 1 to 70 characters",
    )))
}

/// Parts are read in turn: `next_part`, then `next_chunk` until it gives `None`.
pub(crate) struct Multipart<B> {
    body: B,
    buffer: BytesMut,
    /// `\r\n--boundary`; the first one is found because the buffer starts with `\r\n`.
    delimiter: Vec<u8>,
    read: usize,
    limit: usize,
    in_part: bool,
    ended: bool,
}

impl<B: Body<Data = Bytes> + Unpin> Multipart<B>
where
    B::Error: std::fmt::Display,
{
    pub(crate) fn new(body: B, boundary: &str, limit: usize) -> Self {
        Self {
            body,
            buffer: BytesMut::from(&b"\r\n"[..]),
            delimiter: [b"\r\n--", boundary.as_bytes()].concat(),
            read: 0,
            limit,
            in_part: false,
            ended: false,
        }
    }

    pub(crate) fn into_body(self) -> B {
        self.body
    }

    pub(crate) async fn next_part(&mut self) -> Result<Option<PartHead>, Stop> {
        while self.in_part {
            self.next_chunk().await?;
        }
        if self.ended {
            return Ok(None);
        }
        self.fill_to(self.delimiter.len() + 2).await?;
        if !self.buffer.starts_with(&self.delimiter) {
            return Err(Stop::Malformed("it does not start with its boundary"));
        }
        self.buffer.advance(self.delimiter.len());
        if self.buffer.starts_with(b"--") {
            self.ended = true;
            return Ok(None);
        }
        if !self.buffer.starts_with(b"\r\n") {
            return Err(Stop::Malformed(
                "a boundary is not followed by a line break",
            ));
        }
        self.buffer.advance(2);
        let head = self.head().await?;
        self.in_part = true;
        parse_head(&head).map(Some)
    }

    pub(crate) async fn next_chunk(&mut self) -> Result<Option<Bytes>, Stop> {
        if !self.in_part {
            return Ok(None);
        }
        loop {
            if let Some(index) = find(&self.buffer, &self.delimiter) {
                self.in_part = false;
                return Ok((index > 0).then(|| self.buffer.split_to(index).freeze()));
            }
            let safe = self.buffer.len().saturating_sub(self.delimiter.len() - 1);
            if safe > 0 {
                return Ok(Some(self.buffer.split_to(safe).freeze()));
            }
            if !self.more().await? {
                return Err(Stop::Malformed("it ends inside a part"));
            }
        }
    }

    async fn head(&mut self) -> Result<Bytes, Stop> {
        let mut searched = 0;
        loop {
            if let Some(index) = find(&self.buffer[searched..], b"\r\n\r\n") {
                let head = self.buffer.split_to(searched + index).freeze();
                self.buffer.advance(4);
                return Ok(head);
            }
            if self.buffer.len() > HEAD_LIMIT {
                return Err(Stop::Malformed("a part's headers are too long"));
            }
            searched = self.buffer.len().saturating_sub(3);
            if !self.more().await? {
                return Err(Stop::Malformed("it ends inside a part's headers"));
            }
        }
    }

    async fn fill_to(&mut self, length: usize) -> Result<(), Stop> {
        while self.buffer.len() < length {
            if !self.more().await? {
                return Err(Stop::Malformed("it ends before its closing boundary"));
            }
        }
        Ok(())
    }

    async fn more(&mut self) -> Result<bool, Stop> {
        loop {
            let Some(frame) = self.body.frame().await else {
                return Ok(false);
            };
            let frame = frame.map_err(|error| Stop::Unread(error.to_string()))?;
            let Ok(data) = frame.into_data() else {
                continue;
            };
            self.read += data.len();
            if self.read > self.limit {
                return Err(Stop::TooLarge);
            }
            self.buffer.extend_from_slice(&data);
            return Ok(true);
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let first = *needle.first()?;
    let mut start = 0;
    while let Some(offset) = haystack
        .get(start..)?
        .iter()
        .position(|byte| *byte == first)
    {
        let at = start + offset;
        if haystack.get(at..at + needle.len())? == needle {
            return Some(at);
        }
        start = at + 1;
    }
    None
}

fn parse_head(head: &[u8]) -> Result<PartHead, Stop> {
    let head =
        std::str::from_utf8(head).map_err(|_| Stop::Malformed("a part's headers are not UTF-8"))?;
    let mut disposition = None;
    let mut content_type = None;
    for line in head.split("\r\n") {
        let (name, value) = line
            .split_once(':')
            .ok_or(Stop::Malformed("a part's header has no `:`"))?;
        if name.eq_ignore_ascii_case("content-disposition") {
            disposition = Some(value.trim());
        } else if name.eq_ignore_ascii_case("content-type") {
            content_type = Some(value.trim().to_owned());
        }
    }
    let disposition = disposition.ok_or(Stop::Malformed("a part has no Content-Disposition"))?;
    let (kind, parameters) = disposition.split_once(';').unwrap_or((disposition, ""));
    if !kind.trim().eq_ignore_ascii_case("form-data") {
        return Err(Stop::Malformed("a part is not `form-data`"));
    }
    let mut parameters = parameters_of(parameters);
    let mut take = |wanted: &str| {
        let index = parameters
            .iter()
            .position(|(name, _)| name.eq_ignore_ascii_case(wanted))?;
        Some(parameters.swap_remove(index).1)
    };
    let name = take("name").ok_or(Stop::Malformed("a part has no name"))?;
    Ok(PartHead {
        name,
        filename: take("filename"),
        content_type,
    })
}

/// `; name=value` pairs, a value quoted or not; a quoted one may hold `;`.
fn parameters_of(text: &str) -> Vec<(String, String)> {
    let mut parameters = Vec::new();
    let mut rest = text;
    loop {
        rest = rest.trim_start_matches([';', ' ', '\t']);
        let Some((name, after)) = rest.split_once('=') else {
            return parameters;
        };
        let (value, after) = if let Some(quoted) = after.strip_prefix('"') {
            quoted_string(quoted)
        } else {
            let end = after.find(';').unwrap_or(after.len());
            (after[..end].trim().to_owned(), &after[end..])
        };
        parameters.push((name.trim().to_owned(), value));
        rest = after;
    }
}

/// Browsers send `"` in a name as `%22` and never escape with a backslash, so a Windows path
/// stays whole (WHATWG HTML, "multipart/form-data encoding algorithm").
fn quoted_string(text: &str) -> (String, &str) {
    match text.split_once('"') {
        Some((value, after)) => (value.to_owned(), after),
        None => (text.to_owned(), ""),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::convert::Infallible;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use hyper::body::Frame;

    use super::*;

    struct Pieces(VecDeque<Bytes>);

    impl Body for Pieces {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
            Poll::Ready(self.0.pop_front().map(|piece| Ok(Frame::data(piece))))
        }
    }

    const BODY: &str = "--XyZ\r\n\
        Content-Disposition: form-data; name=\"title\"\r\n\r\n\
        Two words\r\n\
        --XyZ\r\n\
        Content-Disposition: form-data; name=\"photo\"; filename=\"C:\\a;b %22c%22.png\"\r\n\
        Content-Type: image/png\r\n\r\n\
        \r\n--XyX\r\n--Xy x--XyZ\r\n\
        --XyZ--\r\n";

    type Read = Vec<(PartHead, Vec<u8>)>;

    async fn read(pieces: VecDeque<Bytes>, limit: usize) -> Result<Read, Stop> {
        let mut multipart = Multipart::new(Pieces(pieces), "XyZ", limit);
        let mut parts = Vec::new();
        while let Some(head) = multipart.next_part().await? {
            let mut data = Vec::new();
            while let Some(chunk) = multipart.next_chunk().await? {
                assert!(!chunk.is_empty());
                data.extend_from_slice(&chunk);
            }
            parts.push((head, data));
        }
        Ok(parts)
    }

    fn split_every(body: &str, size: usize) -> VecDeque<Bytes> {
        body.as_bytes()
            .chunks(size)
            .map(Bytes::copy_from_slice)
            .collect()
    }

    fn expected() -> Read {
        vec![
            (
                PartHead {
                    name: "title".to_owned(),
                    filename: None,
                    content_type: None,
                },
                b"Two words".to_vec(),
            ),
            (
                PartHead {
                    name: "photo".to_owned(),
                    filename: Some("C:\\a;b %22c%22.png".to_owned()),
                    content_type: Some("image/png".to_owned()),
                },
                b"\r\n--XyX\r\n--Xy x--XyZ".to_vec(),
            ),
        ]
    }

    #[tokio::test]
    async fn reads_fields_and_files_however_the_body_is_split() {
        for size in [1, 2, 3, 5, 7, 11, 64, BODY.len()] {
            assert_eq!(
                read(split_every(BODY, size), 1024).await.unwrap(),
                expected(),
                "{size}"
            );
        }
    }

    #[tokio::test]
    async fn a_part_left_unread_is_skipped() {
        let mut multipart = Multipart::new(Pieces(split_every(BODY, 4)), "XyZ", 1024);

        assert_eq!(multipart.next_part().await.unwrap().unwrap().name, "title");
        assert_eq!(multipart.next_part().await.unwrap().unwrap().name, "photo");
        assert_eq!(multipart.next_part().await.unwrap(), None);
        assert_eq!(multipart.next_part().await.unwrap(), None);
    }

    #[tokio::test]
    async fn stops_past_its_limit_and_at_broken_bodies() {
        assert_eq!(
            read(split_every(BODY, 16), BODY.len() - 1).await,
            Err(Stop::TooLarge)
        );
        for broken in [
            "",
            "--Other\r\n",
            "--XyZ\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\nno end",
            "--XyZ\r\nContent-Disposition: attachment; name=\"a\"\r\n\r\nx\r\n--XyZ--",
            "--XyZ\r\nContent-Type: text/plain\r\n\r\nx\r\n--XyZ--",
            "--XyZ\r\nContent-Disposition: form-data\r\n\r\nx\r\n--XyZ--",
            "--XyZ\r\nno colon\r\n\r\nx\r\n--XyZ--",
            "--XyZ\r\nContent-Disposition: form-data; name=\"a\"\r\n",
        ] {
            assert!(
                matches!(
                    read(split_every(broken, 3), 1024).await,
                    Err(Stop::Malformed(_))
                ),
                "{broken:?}"
            );
        }
        let long_head = format!("--XyZ\r\nX: {}\r\n\r\n", "a".repeat(HEAD_LIMIT));
        assert!(matches!(
            read(split_every(&long_head, 512), 64 * 1024).await,
            Err(Stop::Malformed(_))
        ));
    }

    #[test]
    fn finds_the_boundary_of_a_form_data_body_only() {
        let headers = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_TYPE, value.parse().unwrap());
            headers
        };
        assert_eq!(
            boundary(&headers("Multipart/Form-Data; boundary=\"a b;c\"")),
            Some(Ok("a b;c".to_owned()))
        );
        assert_eq!(
            boundary(&headers("multipart/form-data; charset=utf-8; boundary=XyZ")),
            Some(Ok("XyZ".to_owned()))
        );
        assert!(matches!(
            boundary(&headers("multipart/form-data")),
            Some(Err(Stop::Malformed(_)))
        ));
        let too_long = format!("multipart/form-data; boundary={}", "x".repeat(71));
        assert!(matches!(boundary(&headers(&too_long)), Some(Err(_))));
        assert_eq!(
            boundary(&headers("application/x-www-form-urlencoded")),
            None
        );
        assert_eq!(boundary(&HeaderMap::new()), None);
    }
}
