//! RFC 6455 frames as a server reads and writes them: no extensions, text messages only.

use std::io;

use bytes::Bytes;
use http::header::{CONNECTION, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_VERSION, UPGRADE};
use http::{HeaderMap, Method};
use sofie_core::encoding::base64;
use tokio::io::{AsyncRead, AsyncReadExt};

/// The largest route body by default, which a message carries.
pub(crate) const MESSAGE_LIMIT: usize = crate::routes::BODY_LIMIT;
const ACCEPT_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const CONTROL_LIMIT: usize = 125;

const CONTINUATION: u8 = 0x0;
const TEXT: u8 = 0x1;
const BINARY: u8 = 0x2;
const CLOSE: u8 = 0x8;
const PING: u8 = 0x9;
const PONG: u8 = 0xA;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HandshakeError {
    NotAnUpgrade,
    /// Answered `426` naming the version spoken.
    UnsupportedVersion,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Handshake {
    /// HTTP/1.1, answered `101` with this `Sec-WebSocket-Accept`.
    Upgrade(String),
    /// HTTP/2's extended CONNECT (RFC 8441), answered `200`; it carries no key.
    Connect,
}

pub(crate) fn handshake(method: &Method, headers: &HeaderMap) -> Result<Handshake, HandshakeError> {
    if method == Method::CONNECT {
        return speaks_version_13(headers).map(|()| Handshake::Connect);
    }
    accept(headers).map(Handshake::Upgrade)
}

fn speaks_version_13(headers: &HeaderMap) -> Result<(), HandshakeError> {
    match headers.get(SEC_WEBSOCKET_VERSION) {
        Some(version) if version == "13" => Ok(()),
        _ => Err(HandshakeError::UnsupportedVersion),
    }
}

fn accept(headers: &HeaderMap) -> Result<String, HandshakeError> {
    let upgrades =
        has_token(headers, &UPGRADE, "websocket") && has_token(headers, &CONNECTION, "upgrade");
    if !upgrades {
        return Err(HandshakeError::NotAnUpgrade);
    }
    speaks_version_13(headers)?;
    let key = headers
        .get(SEC_WEBSOCKET_KEY)
        .and_then(|key| key.to_str().ok())
        .filter(|key| base64::decode(&base64::STANDARD, key).is_some_and(|nonce| nonce.len() == 16))
        .ok_or(HandshakeError::NotAnUpgrade)?;
    let mut context = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
    context.update(key.as_bytes());
    context.update(ACCEPT_GUID);
    Ok(base64::encode_padded(
        &base64::STANDARD,
        context.finish().as_ref(),
    ))
}

fn has_token(headers: &HeaderMap, name: &http::HeaderName, token: &str) -> bool {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|listed| listed.trim().eq_ignore_ascii_case(token))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloseCode {
    Normal,
    GoingAway,
    Protocol,
    Unsupported,
    NotText,
    Policy,
    TooLarge,
    /// Private use (4000–4999): the client opens a new socket at once, carrying its current
    /// cookie, so an ended session stops hearing.
    Reconnect,
}

impl CloseCode {
    fn number(self) -> u16 {
        match self {
            Self::Normal => 1000,
            Self::GoingAway => 1001,
            Self::Protocol => 1002,
            Self::Unsupported => 1003,
            Self::NotText => 1007,
            Self::Policy => 1008,
            Self::TooLarge => 1009,
            Self::Reconnect => 4000,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Received {
    Text(String),
    Ping(Bytes),
    Pong,
    Close,
}

#[derive(Debug)]
pub(crate) enum ReadError {
    Gone,
    Violated(CloseCode),
}

impl From<io::Error> for ReadError {
    fn from(_: io::Error) -> Self {
        Self::Gone
    }
}

struct Frame {
    is_final: bool,
    opcode: u8,
    payload: Vec<u8>,
}

pub(crate) struct Reader<R> {
    io: R,
    partial: Option<Vec<u8>>,
}

impl<R: AsyncRead + Unpin> Reader<R> {
    pub(crate) fn new(io: R) -> Self {
        Self { io, partial: None }
    }

    pub(crate) async fn next(&mut self) -> Result<Received, ReadError> {
        loop {
            let frame = self.frame().await?;
            let payload = match (frame.opcode, self.partial.as_mut()) {
                (CLOSE, _) => return Ok(Received::Close),
                (PING, _) => return Ok(Received::Ping(Bytes::from(frame.payload))),
                (PONG, _) => return Ok(Received::Pong),
                (BINARY, None) => return Err(ReadError::Violated(CloseCode::Unsupported)),
                (TEXT, None) => frame.payload,
                (CONTINUATION, Some(partial)) => {
                    partial.extend_from_slice(&frame.payload);
                    if !frame.is_final {
                        continue;
                    }
                    self.partial.take().unwrap_or_default()
                }
                _ => return Err(ReadError::Violated(CloseCode::Protocol)),
            };
            if !frame.is_final {
                self.partial = Some(payload);
                continue;
            }
            return String::from_utf8(payload)
                .map(Received::Text)
                .map_err(|_| ReadError::Violated(CloseCode::NotText));
        }
    }

    async fn frame(&mut self) -> Result<Frame, ReadError> {
        let mut head = [0u8; 2];
        self.io.read_exact(&mut head).await?;
        let is_final = head[0] & 0x80 != 0;
        let opcode = head[0] & 0x0F;
        let is_masked = head[1] & 0x80 != 0;
        let is_control = opcode & 0x08 != 0;
        // No extension was negotiated, and a client masks every frame (RFC 6455 section 5.1).
        if head[0] & 0x70 != 0 || !is_masked {
            return Err(ReadError::Violated(CloseCode::Protocol));
        }
        let length = match head[1] & 0x7F {
            126 => u64::from(self.io.read_u16().await?),
            127 => self.io.read_u64().await?,
            short => u64::from(short),
        };
        if is_control && (!is_final || length > CONTROL_LIMIT as u64) {
            return Err(ReadError::Violated(CloseCode::Protocol));
        }
        let buffered = self.partial.as_ref().map_or(0, Vec::len);
        let length = usize::try_from(length)
            .ok()
            .filter(|length| buffered + length <= MESSAGE_LIMIT)
            .ok_or(ReadError::Violated(CloseCode::TooLarge))?;
        let mut mask = [0u8; 4];
        self.io.read_exact(&mut mask).await?;
        let mut payload = vec![0u8; length];
        self.io.read_exact(&mut payload).await?;
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
        Ok(Frame {
            is_final,
            opcode,
            payload,
        })
    }
}

pub(crate) fn text(message: &str) -> Bytes {
    frame(TEXT, message.as_bytes())
}

pub(crate) fn ping() -> Bytes {
    frame(PING, b"")
}

pub(crate) fn pong(payload: &[u8]) -> Bytes {
    frame(PONG, payload)
}

pub(crate) fn close(code: CloseCode, reason: &str) -> Bytes {
    let mut payload = code.number().to_be_bytes().to_vec();
    payload.extend_from_slice(reason.as_bytes());
    payload.truncate(CONTROL_LIMIT);
    frame(CLOSE, &payload)
}

fn frame(opcode: u8, payload: &[u8]) -> Bytes {
    let mut frame = Vec::with_capacity(payload.len() + 10);
    frame.push(0x80 | opcode);
    match payload.len() {
        #[expect(clippy::cast_possible_truncation, reason = "matched below 126")]
        short @ 0..126 => frame.push(short as u8),
        #[expect(clippy::cast_possible_truncation, reason = "matched below 65 536")]
        medium @ 126..=0xFFFF => {
            frame.push(126);
            frame.extend_from_slice(&(medium as u16).to_be_bytes());
        }
        long => {
            frame.push(127);
            frame.extend_from_slice(&(long as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(payload);
    Bytes::from(frame)
}

#[cfg(test)]
mod tests {
    use http::HeaderValue;

    use super::*;

    fn masked(first: u8, payload: &[u8]) -> Vec<u8> {
        let mask = [0x37, 0xFA, 0x21, 0x3D];
        let mut frame = vec![first];
        match payload.len() {
            short @ 0..126 => frame.push(0x80 | u8::try_from(short).unwrap()),
            medium @ 126..=0xFFFF => {
                frame.push(0x80 | 0x7E);
                frame.extend_from_slice(&u16::try_from(medium).unwrap().to_be_bytes());
            }
            long => {
                frame.push(0x80 | 0x7F);
                frame.extend_from_slice(&u64::try_from(long).unwrap().to_be_bytes());
            }
        }
        frame.extend_from_slice(&mask);
        frame.extend(
            payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ mask[index % 4]),
        );
        frame
    }

    async fn read(bytes: Vec<u8>) -> Result<Received, ReadError> {
        Reader::new(bytes.as_slice()).next().await
    }

    fn upgrade_headers(key: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
        headers.insert(CONNECTION, HeaderValue::from_static("keep-alive, Upgrade"));
        headers.insert(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"));
        headers.insert(SEC_WEBSOCKET_KEY, HeaderValue::from_str(key).unwrap());
        headers
    }

    #[test]
    fn the_accept_key_is_the_rfc_6455_example() {
        assert_eq!(
            accept(&upgrade_headers("dGhlIHNhbXBsZSBub25jZQ==")),
            Ok("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=".to_owned())
        );
    }

    #[test]
    fn an_http2_connect_needs_only_version_13() {
        let mut headers = HeaderMap::new();
        headers.insert(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"));

        assert_eq!(
            handshake(&Method::CONNECT, &headers),
            Ok(Handshake::Connect)
        );
        assert_eq!(
            handshake(&Method::CONNECT, &HeaderMap::new()),
            Err(HandshakeError::UnsupportedVersion)
        );
    }

    #[test]
    fn a_handshake_without_a_sixteen_byte_key_or_version_13_is_refused() {
        assert_eq!(
            accept(&upgrade_headers("c2hvcnQ=")),
            Err(HandshakeError::NotAnUpgrade)
        );
        let mut old = upgrade_headers("dGhlIHNhbXBsZSBub25jZQ==");
        old.insert(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("8"));
        assert_eq!(accept(&old), Err(HandshakeError::UnsupportedVersion));
        let mut plain = upgrade_headers("dGhlIHNhbXBsZSBub25jZQ==");
        plain.remove(UPGRADE);
        assert_eq!(accept(&plain), Err(HandshakeError::NotAnUpgrade));
    }

    #[tokio::test]
    async fn a_fragmented_text_message_is_joined_around_a_ping() {
        let mut bytes = masked(TEXT, b"Hel");
        bytes.extend(masked(0x80 | PING, b"hi"));
        bytes.extend(masked(0x80 | CONTINUATION, b"lo"));
        let mut reader = Reader::new(bytes.as_slice());

        assert_eq!(
            reader.next().await.unwrap(),
            Received::Ping(Bytes::from_static(b"hi"))
        );
        assert_eq!(
            reader.next().await.unwrap(),
            Received::Text("Hello".to_owned())
        );
    }

    #[tokio::test]
    async fn frames_a_server_must_refuse_close_the_socket_with_their_code() {
        let unmasked = vec![0x80 | TEXT, 0x02, b'h', b'i'];
        let cases = [
            (unmasked, CloseCode::Protocol),
            (masked(0x80 | BINARY, b"x"), CloseCode::Unsupported),
            (masked(0x80 | TEXT, &[0xFF, 0xFE]), CloseCode::NotText),
            (masked(0x80 | 0x40 | TEXT, b"x"), CloseCode::Protocol),
            (masked(PING, b""), CloseCode::Protocol),
            (masked(0x80 | CONTINUATION, b"x"), CloseCode::Protocol),
            (
                masked(0x80 | TEXT, &vec![b'a'; MESSAGE_LIMIT + 1]),
                CloseCode::TooLarge,
            ),
        ];
        for (bytes, code) in cases {
            let outcome = read(bytes).await;
            assert!(
                matches!(outcome, Err(ReadError::Violated(found)) if found == code),
                "{outcome:?}"
            );
        }
    }

    #[test]
    fn a_server_frame_is_unmasked_with_the_shortest_length() {
        assert_eq!(&text("hi")[..], &[0x81, 0x02, b'h', b'i']);
        let medium = text(&"a".repeat(200));
        assert_eq!(&medium[..4], &[0x81, 126, 0, 200]);
        assert_eq!(
            &close(CloseCode::Reconnect, "")[..],
            &[0x88, 0x02, 0x0F, 0xA0]
        );
    }
}
