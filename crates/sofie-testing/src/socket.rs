use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::TestApp;
use crate::request::TestSession;

/// Long enough for a commit's announcement to reach the socket, short enough that a test
/// expecting no answer is quick.
const ANSWER_WAIT: Duration = Duration::from_secs(1);
const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const TEXT: u8 = 0x1;
const CLOSE: u8 = 0x8;
const PING: u8 = 0x9;

/// A browser's socket to `/_sofie/socket`, from the app's own origin.
pub struct TestSocket {
    stream: TcpStream,
    next_id: u64,
}

impl TestSocket {
    /// Signed in by no one; `TestCaller::socket` opens one as its principal.
    pub async fn open(app: &TestApp) -> Self {
        Self::connect(app.origin(), None).await
    }

    pub(crate) async fn connect(origin: &str, session: Option<&TestSession>) -> Self {
        let address = origin.trim_start_matches("http://");
        let mut stream = TcpStream::connect(address)
            .await
            .expect("connecting to the test app");
        let cookie = session.map_or(String::new(), |session| {
            format!("Cookie: {}\r\n", session.cookie())
        });
        let handshake = format!(
            "GET /_sofie/socket HTTP/1.1\r\nHost: {address}\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {KEY}\r\n\
             Origin: {origin}\r\n{cookie}\r\n"
        );
        stream
            .write_all(handshake.as_bytes())
            .await
            .expect("sending the handshake");
        let head = read_head(&mut stream).await;
        assert!(
            head.starts_with("HTTP/1.1 101"),
            "the socket was refused; is the app's `socket` feature on?\n{head}"
        );
        Self { stream, next_id: 1 }
    }

    /// Returns the watch's id, which its answers carry.
    pub async fn watch(&mut self, path: &str) -> u64 {
        self.call("GET", path, None).await
    }

    pub async fn call(&mut self, method: &str, path: &str, body: Option<Value>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({ "id": id, "method": method, "path": path });
        if let Some(body) = body {
            message["body"] = body;
        }
        self.send(&message).await;
        id
    }

    pub async fn stop(&mut self, id: u64) {
        self.send(&json!({ "stop": id })).await;
    }

    pub async fn send(&mut self, message: &Value) {
        let frame = masked_text(message.to_string().as_bytes());
        self.stream
            .write_all(&frame)
            .await
            .expect("sending a message");
    }

    /// The next answer, `{ "id", "status", "body" }`, or `None` when none comes within a second.
    pub async fn answer(&mut self) -> Option<Value> {
        let (opcode, payload) = self.frame().await?;
        assert_eq!(opcode, TEXT, "expected an answer, got frame {opcode:#x}");
        Some(serde_json::from_slice(&payload).expect("an answer is JSON"))
    }

    /// The code the server closed the socket with.
    pub async fn closed_with(&mut self) -> Option<u16> {
        let (opcode, payload) = self.frame().await?;
        (opcode == CLOSE && payload.len() >= 2)
            .then(|| u16::from_be_bytes([payload[0], payload[1]]))
    }

    async fn frame(&mut self) -> Option<(u8, Vec<u8>)> {
        let reading = async {
            loop {
                let mut head = [0; 2];
                self.stream.read_exact(&mut head).await.ok()?;
                let length = match head[1] & 0x7F {
                    126 => usize::from(self.stream.read_u16().await.ok()?),
                    127 => usize::try_from(self.stream.read_u64().await.ok()?).ok()?,
                    short => usize::from(short),
                };
                let mut payload = vec![0; length];
                self.stream.read_exact(&mut payload).await.ok()?;
                if head[0] & 0x0F != PING {
                    return Some((head[0] & 0x0F, payload));
                }
            }
        };
        tokio::time::timeout(ANSWER_WAIT, reading)
            .await
            .ok()
            .flatten()
    }
}

async fn read_head(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        if stream
            .read(&mut byte)
            .await
            .expect("reading the handshake's answer")
            == 0
        {
            break;
        }
        head.push(byte[0]);
    }
    String::from_utf8_lossy(&head).into_owned()
}

/// A client masks every frame (RFC 6455 section 5.3); the key need not be secret in a test.
fn masked_text(payload: &[u8]) -> Vec<u8> {
    let mask = [0x11, 0x22, 0x33, 0x44];
    let mut frame = vec![0x80 | TEXT];
    match payload.len() {
        short @ 0..126 => frame.push(0x80 | u8::try_from(short).expect("matched below 126")),
        medium @ 126..=0xFFFF => {
            frame.push(0x80 | 0x7E);
            frame.extend_from_slice(
                &u16::try_from(medium)
                    .expect("matched below 65 536")
                    .to_be_bytes(),
            );
        }
        long => {
            frame.push(0x80 | 0x7F);
            frame.extend_from_slice(&(long as u64).to_be_bytes());
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
