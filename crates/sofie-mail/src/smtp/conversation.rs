use sofie_core::{Error, Result};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use sofie_core::encoding::base64::{STANDARD, encode_padded};

use crate::Address;
use crate::transport::Refused;

/// Longer than any reply line a server sends, so a hostile one cannot fill memory.
const MAXIMUM_LINE: u64 = 4096;
const MAXIMUM_LINES: usize = 100;

pub(crate) struct Reply {
    code: u16,
    lines: Vec<String>,
}

impl Reply {
    fn is_permanent(&self) -> bool {
        (500..600).contains(&self.code)
    }

    /// The code and the server's words, each word naming an address left out, so it can be logged and stored.
    fn shown(&self) -> String {
        let mut shown = self.code.to_string();
        for word in self.lines.iter().flat_map(|line| line.split_whitespace()) {
            shown.push(' ');
            shown.push_str(if word.contains('@') {
                "[address]"
            } else {
                word
            });
        }
        shown
    }
}

pub(crate) struct Extensions {
    pub(crate) starttls: bool,
    pub(crate) smtputf8: bool,
    pub(crate) auth: Vec<String>,
}

impl Extensions {
    fn read(reply: &Reply) -> Self {
        let mut extensions = Self {
            starttls: false,
            smtputf8: false,
            auth: Vec::new(),
        };
        for line in reply.lines.iter().skip(1) {
            let upper = line.to_ascii_uppercase();
            if upper == "STARTTLS" {
                extensions.starttls = true;
            } else if upper == "SMTPUTF8" {
                extensions.smtputf8 = true;
            } else if let Some(mechanisms) = upper
                .strip_prefix("AUTH ")
                .or_else(|| upper.strip_prefix("AUTH="))
            {
                extensions
                    .auth
                    .extend(mechanisms.split_whitespace().map(str::to_owned));
            }
        }
        extensions
    }

    fn offers(&self, mechanism: &str) -> bool {
        self.auth.iter().any(|offered| offered == mechanism)
    }
}

/// A permanent refusal within the mail transaction is the mail's fault, so it is not retried;
/// one of the session is the server's or the settings', which may be fixed by the next try.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Session,
    Transaction,
}

#[derive(Clone, Copy)]
pub(crate) struct Command {
    name: &'static str,
    expected: &'static [u16],
    stage: Stage,
}

impl Command {
    const fn new(name: &'static str, expected: &'static [u16], stage: Stage) -> Self {
        Self {
            name,
            expected,
            stage,
        }
    }
}

pub(crate) const GREETING: Command = Command::new("its greeting", &[220], Stage::Session);
const EHLO: Command = Command::new("EHLO", &[250], Stage::Session);
pub(crate) const STARTTLS: Command = Command::new("STARTTLS", &[220], Stage::Session);
const AUTH: Command = Command::new("AUTH", &[235], Stage::Session);
const AUTH_STEP: Command = Command::new("AUTH", &[334], Stage::Session);
pub(crate) const MAIL_FROM: Command = Command::new("MAIL FROM", &[250], Stage::Transaction);
const RCPT_TO: Command = Command::new("RCPT TO", &[250, 251], Stage::Transaction);
pub(crate) const DATA: Command = Command::new("DATA", &[354], Stage::Transaction);
const MESSAGE: Command = Command::new("the message", &[250], Stage::Transaction);
const QUIT: Command = Command::new("QUIT", &[221], Stage::Session);

pub(crate) struct Conversation<'h, S> {
    reader: BufReader<S>,
    host: &'h str,
}

impl<'h, S: AsyncRead + AsyncWrite + Unpin + Send> Conversation<'h, S> {
    pub(crate) fn new(stream: S, host: &'h str) -> Self {
        Self {
            reader: BufReader::new(stream),
            host,
        }
    }

    pub(crate) async fn expect(&mut self, command: Command) -> Result<Reply> {
        let reply = self.reply().await?;
        if command.expected.contains(&reply.code) {
            return Ok(reply);
        }
        Err(self.refusal(command, &reply))
    }

    pub(crate) async fn send(&mut self, line: &str, command: Command) -> Result<Reply> {
        self.command(line).await?;
        self.expect(command).await
    }

    /// A refused recipient is returned rather than failing, so the others still get the mail.
    pub(crate) async fn recipient(&mut self, recipient: &Address) -> Result<Option<Refused>> {
        let to = recipient.email().as_str();
        self.command(&format!("RCPT TO:<{to}>")).await?;
        let reply = self.reply().await?;
        if RCPT_TO.expected.contains(&reply.code) {
            return Ok(None);
        }
        Ok(Some(Refused::new(
            recipient.clone(),
            reply.code,
            reply.shown(),
        )))
    }

    pub(crate) async fn hello(&mut self, client: &str) -> Result<Extensions> {
        let reply = self.send(&format!("EHLO {client}"), EHLO).await?;
        Ok(Extensions::read(&reply))
    }

    /// PLAIN when offered, else LOGIN; the secret travels only inside TLS, which the settings require.
    pub(crate) async fn authenticate(
        &mut self,
        extensions: &Extensions,
        username: &str,
        password: &str,
    ) -> Result<()> {
        if [username, password]
            .iter()
            .any(|part| part.contains(['\0', '\r', '\n']))
        {
            return Err(Error::internal_message(
                "the SMTP username or password holds a NUL or line break, which AUTH cannot carry",
            ));
        }
        if extensions.offers("PLAIN") {
            let token = encode_padded(&STANDARD, format!("\0{username}\0{password}").as_bytes());
            self.send(&format!("AUTH PLAIN {token}"), AUTH).await?;
            return Ok(());
        }
        if extensions.offers("LOGIN") {
            self.send("AUTH LOGIN", AUTH_STEP).await?;
            self.send(&encode_padded(&STANDARD, username.as_bytes()), AUTH_STEP)
                .await?;
            self.send(&encode_padded(&STANDARD, password.as_bytes()), AUTH)
                .await?;
            return Ok(());
        }
        Err(Error::Unavailable(format!(
            "the SMTP server {} offers neither AUTH PLAIN nor AUTH LOGIN",
            self.host
        )))
    }

    /// One write: each would otherwise go out as a packet and a TLS record of its own.
    pub(crate) async fn message(&mut self, message: &str) -> Result<()> {
        self.write(&dot_stuffed(message)).await?;
        self.expect(MESSAGE).await.map(drop)
    }

    /// The message was accepted already, so a server closing early changes nothing.
    pub(crate) async fn quit(&mut self) {
        if let Err(error) = self.send("QUIT", QUIT).await {
            tracing::debug!(error = %error, "the SMTP server did not answer QUIT");
        }
    }

    /// Bytes read before TLS but after the server's go-ahead would be taken as sent inside it.
    pub(crate) fn into_stream(self) -> Result<S> {
        if !self.reader.buffer().is_empty() {
            return Err(Error::Unavailable(format!(
                "the SMTP server {} sent more than its STARTTLS reply before TLS began",
                self.host
            )));
        }
        Ok(self.reader.into_inner())
    }

    async fn command(&mut self, line: &str) -> Result<()> {
        self.write(format!("{line}\r\n").as_bytes()).await
    }

    fn refusal(&self, command: Command, reply: &Reply) -> Error {
        let message = format!(
            "the SMTP server {} answered {} with {}",
            self.host,
            command.name,
            reply.shown()
        );
        if command.stage == Stage::Transaction && reply.is_permanent() {
            return Error::Rule(message);
        }
        Error::Unavailable(message)
    }

    async fn reply(&mut self) -> Result<Reply> {
        let mut lines = Vec::new();
        let mut first_code = 0;
        loop {
            let line = self.line().await?;
            let code = reply_code(&line).ok_or_else(|| self.malformed("a reply without a code"))?;
            if lines.is_empty() {
                first_code = code;
            } else if code != first_code {
                return Err(self.malformed("a reply whose lines name different codes"));
            }
            lines.push(line.get(4..).unwrap_or_default().to_owned());
            match line.as_bytes().get(3) {
                None | Some(b' ') => return Ok(Reply { code, lines }),
                Some(b'-') if lines.len() < MAXIMUM_LINES => {}
                Some(b'-') => return Err(self.malformed("a reply of too many lines")),
                Some(_) => return Err(self.malformed("a reply line it could not read")),
            }
        }
    }

    async fn line(&mut self) -> Result<String> {
        let mut bytes = Vec::new();
        let read = (&mut self.reader)
            .take(MAXIMUM_LINE)
            .read_until(b'\n', &mut bytes)
            .await
            .map_err(|error| self.failed(&error))?;
        if read == 0 {
            return Err(Error::Unavailable(format!(
                "the SMTP server {} closed the connection",
                self.host
            )));
        }
        if !bytes.ends_with(b"\n") {
            return Err(self.malformed("a reply line longer than 4096 bytes"));
        }
        let line = String::from_utf8_lossy(&bytes);
        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let stream = self.reader.get_mut();
        let written = match stream.write_all(bytes).await {
            Ok(()) => stream.flush().await,
            failed => failed,
        };
        written.map_err(|error| self.failed(&error))
    }

    fn failed(&self, error: &std::io::Error) -> Error {
        Error::Unavailable(format!("talking to the SMTP server {}: {error}", self.host))
    }

    fn malformed(&self, what: &str) -> Error {
        Error::Unavailable(format!("the SMTP server {} sent {what}", self.host))
    }
}

fn dot_stuffed(message: &str) -> Vec<u8> {
    let mut data = Vec::with_capacity(message.len() + 5);
    for line in message.split_inclusive("\r\n") {
        if line.starts_with('.') {
            data.push(b'.');
        }
        data.extend_from_slice(line.as_bytes());
    }
    if !message.ends_with("\r\n") {
        data.extend_from_slice(b"\r\n");
    }
    data.extend_from_slice(b".\r\n");
    data
}

/// Exactly three digits (RFC 5321 section 4.2), so `+25` is no code.
fn reply_code(line: &str) -> Option<u16> {
    let digits = line.get(..3)?;
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    #[tokio::test]
    async fn a_line_starting_with_a_dot_is_stuffed_and_the_data_ends_with_one() {
        let (client, mut server) = tokio::io::duplex(1024);
        server.write_all(b"250 queued\r\n").await.unwrap();
        let mut conversation = Conversation::new(client, "localhost");

        conversation
            .message(".hidden\r\nline\r\n..two\r\nlast")
            .await
            .unwrap();
        drop(conversation);

        let mut wire = String::new();
        server.read_to_string(&mut wire).await.unwrap();
        assert_eq!(wire, "..hidden\r\nline\r\n...two\r\nlast\r\n.\r\n");
    }

    #[tokio::test]
    async fn a_reply_of_several_lines_is_read_as_one() {
        let (client, mut server) = tokio::io::duplex(1024);
        server
            .write_all(b"250-mail.test\r\n250-STARTTLS\r\n250-AUTH PLAIN LOGIN\r\n250 8BITMIME\r\n")
            .await
            .unwrap();
        let mut conversation = Conversation::new(client, "localhost");

        let extensions = conversation.hello("localhost").await.unwrap();

        assert!(extensions.starttls);
        assert_eq!(extensions.auth, ["PLAIN", "LOGIN"]);
    }

    #[test]
    fn a_reply_code_is_three_digits() {
        assert_eq!(reply_code("250 ok"), Some(250));
        assert_eq!(reply_code("+25 ok"), None);
        assert_eq!(reply_code("25"), None);
    }

    #[tokio::test]
    async fn a_reply_whose_lines_disagree_is_refused() {
        let (client, mut server) = tokio::io::duplex(1024);
        server
            .write_all(b"250-first\r\n550 second\r\n")
            .await
            .unwrap();
        let mut conversation = Conversation::new(client, "localhost");

        let error = conversation.expect(GREETING).await.err().unwrap();

        assert!(error.to_string().contains("different codes"), "{error}");
    }

    #[tokio::test]
    async fn an_endless_line_is_refused() {
        let (client, mut server) = tokio::io::duplex(8192);
        server.write_all(&[b'2'; 5000]).await.unwrap();
        let mut conversation = Conversation::new(client, "localhost");

        let error = conversation.expect(GREETING).await.err().unwrap();

        assert!(
            error.to_string().contains("longer than 4096 bytes"),
            "{error}"
        );
    }
}
