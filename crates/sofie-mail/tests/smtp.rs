//! A fake server on a local socket, with a certificate only the test's own root signs.

use std::sync::{Arc, Mutex};

use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sofie_core::encoding::base64::{self, STANDARD};
use sofie_core::{Error, Result, Timestamp, unpoisoned};
use sofie_mail::testing::{Refused, body, smtp};
use sofie_mail::{Address, Mail, Security, SmtpSettings};
use sofie_values::Email;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

const ROOT: &[u8] = include_bytes!("../../sofie-client/tests/certificates/root.der");
const SERVER_CERTIFICATE: &[u8] =
    include_bytes!("../../sofie-client/tests/certificates/server.der");
const SERVER_KEY: &[u8] = include_bytes!("../../sofie-client/tests/certificates/server.key.der");
const KEY: &str = "0190c3e2-0000-7000-8000-000000000001";

#[derive(Clone, Copy, Default)]
struct Script {
    encryption: Encryption,
    auth: &'static str,
    /// The addresses `RCPT TO` is answered with `refusal`.
    refused: &'static [&'static str],
    refusal: &'static str,
    /// What the end of the message is answered with, `250 queued` when empty.
    message_reply: &'static str,
    /// Never answers QUIT, as a tarpitting server would.
    silent_on_quit: bool,
    /// Offers addresses beyond ASCII (RFC 6531).
    smtputf8: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Encryption {
    #[default]
    None,
    StartTls,
    /// Sends a line right after its STARTTLS reply, as an attacker on the path would.
    StartTlsInjected,
    Implicit,
}

#[derive(Default)]
struct Seen {
    commands: Vec<String>,
    message: String,
    encrypted_commands: Vec<String>,
}

struct Server {
    port: u16,
    seen: Arc<Mutex<Seen>>,
}

impl Server {
    async fn start(script: Script) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Seen::default()));
        let serving = Arc::clone(&seen);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            serve(stream, script, &serving).await;
        });
        Self { port, seen }
    }

    async fn send(&self, security: Security, mail: &Mail) -> Result<Vec<Refused>> {
        self.send_as(SmtpSettings::new("localhost", self.port, security), mail)
            .await
    }

    async fn send_logged_in(&self, security: Security) -> Result<Vec<Refused>> {
        let settings = SmtpSettings::new("localhost", self.port, security)
            .with_credentials("mailer", "secret password");
        self.send_as(settings, &mail()).await
    }

    async fn send_as(&self, settings: SmtpSettings, mail: &Mail) -> Result<Vec<Refused>> {
        let now = Timestamp::from_second(1_700_000_000).unwrap();
        smtp(settings, ROOT).unwrap().send(mail, KEY, now).await
    }

    fn seen(&self) -> std::sync::MutexGuard<'_, Seen> {
        unpoisoned::lock(&self.seen)
    }
}

fn acceptor() -> TlsAcceptor {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(SERVER_KEY.to_vec()));
    let config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(SERVER_CERTIFICATE.to_vec())], key)
        .unwrap();
    TlsAcceptor::from(Arc::new(config))
}

async fn serve(stream: TcpStream, script: Script, seen: &Mutex<Seen>) {
    let plain = if script.encryption == Encryption::Implicit {
        Some(stream)
    } else {
        converse(stream, script, false, seen).await
    };
    let Some(stream) = plain else {
        return;
    };
    if let Ok(stream) = acceptor().accept(stream).await {
        converse(stream, script, true, seen).await;
    }
}

async fn converse<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    script: Script,
    encrypted: bool,
    seen: &Mutex<Seen>,
) -> Option<S> {
    let mut stream = BufReader::new(stream);
    if !encrypted || script.encryption == Encryption::Implicit {
        reply(&mut stream, "220 localhost ESMTP fake").await;
    }
    loop {
        let mut line = String::new();
        if stream.read_line(&mut line).await.unwrap_or_default() == 0 {
            return None;
        }
        let command = line.trim_end().to_owned();
        {
            let mut seen = unpoisoned::lock(seen);
            seen.commands.push(command.clone());
            if encrypted {
                seen.encrypted_commands.push(command.clone());
            }
        }
        let verb = command
            .split([' ', ':'])
            .next()
            .unwrap_or_default()
            .to_uppercase();
        match verb.as_str() {
            "EHLO" => {
                let mut lines = vec!["250-localhost".to_owned()];
                let offers_starttls = matches!(
                    script.encryption,
                    Encryption::StartTls | Encryption::StartTlsInjected
                );
                if offers_starttls && !encrypted {
                    lines.push("250-STARTTLS".to_owned());
                }
                if encrypted && !script.auth.is_empty() {
                    lines.push(format!("250-AUTH {}", script.auth));
                }
                if script.smtputf8 {
                    lines.push("250-SMTPUTF8".to_owned());
                }
                lines.push("250 8BITMIME".to_owned());
                reply(&mut stream, &lines.join("\r\n")).await;
            }
            "STARTTLS" => {
                let answer = if script.encryption == Encryption::StartTlsInjected {
                    "220 go ahead\r\n250 injected"
                } else {
                    "220 go ahead"
                };
                reply(&mut stream, answer).await;
                return Some(stream.into_inner());
            }
            "AUTH" if command == "AUTH LOGIN" => reply(&mut stream, "334 VXNlcm5hbWU6").await,
            "AUTH" => reply(&mut stream, "235 2.7.0 accepted").await,
            "RCPT"
                if script
                    .refused
                    .iter()
                    .any(|refused| command.contains(refused)) =>
            {
                let address = command.trim_start_matches("RCPT TO:");
                reply(&mut stream, &script.refusal.replace("{address}", address)).await;
            }
            "MAIL" | "RCPT" => reply(&mut stream, "250 ok").await,
            "DATA" => {
                reply(&mut stream, "354 go on").await;
                let message = read_data(&mut stream).await;
                unpoisoned::lock(seen).message = message;
                let answer = if script.message_reply.is_empty() {
                    "250 queued"
                } else {
                    script.message_reply
                };
                reply(&mut stream, answer).await;
            }
            "QUIT" if script.silent_on_quit => std::future::pending::<()>().await,
            "QUIT" => {
                reply(&mut stream, "221 bye").await;
                return None;
            }
            // The second and third steps of AUTH LOGIN: the username and the password.
            _ if command.starts_with("AUTH LOGIN") || is_base64(&command) => {
                let answer = if seen_logins(seen) == 1 {
                    "334 UGFzc3dvcmQ6"
                } else {
                    "235 ok"
                };
                reply(&mut stream, answer).await;
            }
            _ => reply(&mut stream, "500 unknown").await,
        }
    }
}

fn seen_logins(seen: &Mutex<Seen>) -> usize {
    let seen = unpoisoned::lock(seen);
    let start = seen
        .commands
        .iter()
        .position(|command| command == "AUTH LOGIN")
        .unwrap_or_default();
    seen.commands.len() - start - 1
}

fn is_base64(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "+/=".contains(character))
}

async fn reply<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut BufReader<S>, text: &str) {
    let written = format!("{text}\r\n");
    let _ = stream.get_mut().write_all(written.as_bytes()).await;
    let _ = stream.get_mut().flush().await;
}

async fn read_data<S: AsyncRead + Unpin>(stream: &mut BufReader<S>) -> String {
    let mut message = String::new();
    loop {
        let mut line = String::new();
        stream.read_line(&mut line).await.unwrap();
        if line == ".\r\n" {
            return message;
        }
        message.push_str(line.strip_prefix('.').unwrap_or(&line));
    }
}

fn address(text: &str) -> Address {
    Address::new(&Email::parse(text).unwrap()).unwrap()
}

fn mail() -> Mail {
    let html = "<p>Your order ships today.</p><p>Grüße</p>";
    Mail::new(
        address("ada@example.com"),
        "Your order: on its way ✓",
        body(html),
    )
    .unwrap()
    .also_to(address("bob@example.com"))
    .sent_from(address("shop@example.com").named("Shop").unwrap())
}

fn decode_base64(text: &str) -> Vec<u8> {
    base64::decode(&STANDARD, &text.replace("\r\n", "")).unwrap()
}

fn text_part(message: &str) -> String {
    let start = message.find("text/plain").unwrap();
    let content = &message[start..];
    let content = &content[content.find("\r\n\r\n").unwrap() + 4..];
    let content = &content[..content.find("--").unwrap()];
    String::from_utf8(decode_base64(content)).unwrap()
}

#[tokio::test]
async fn a_mail_is_delivered_in_the_clear_to_a_local_relay() {
    let server = Server::start(Script::default()).await;

    server.send(Security::None, &mail()).await.unwrap();

    let seen = server.seen();
    assert_eq!(
        seen.commands,
        [
            "EHLO localhost",
            "MAIL FROM:<shop@example.com>",
            "RCPT TO:<ada@example.com>",
            "RCPT TO:<bob@example.com>",
            "DATA",
            "QUIT",
        ]
    );
    assert!(
        seen.message
            .contains("\r\nTo: ada@example.com,\r\n bob@example.com\r\n")
    );
    assert!(seen.message.contains("\r\nSubject: =?UTF-8?B?"));
    assert!(
        seen.message
            .contains(&format!("Message-ID: <{KEY}@example.com>"))
    );
    assert_eq!(
        text_part(&seen.message),
        "Your order ships today.\r\n\r\nGrüße"
    );
}

#[tokio::test]
async fn a_server_that_took_the_mail_but_never_answers_quit_still_counts_it_sent() {
    let server = Server::start(Script {
        silent_on_quit: true,
        ..Script::default()
    })
    .await;

    let sent = server.send(Security::None, &mail()).await;

    assert!(sent.unwrap().is_empty());
    assert!(
        server
            .seen()
            .commands
            .ends_with(&["DATA".to_owned(), "QUIT".to_owned()])
    );
}

#[tokio::test]
async fn starttls_comes_before_the_credentials_and_auth_plain_carries_them() {
    let script = Script {
        encryption: Encryption::StartTls,
        auth: "LOGIN PLAIN",
        ..Script::default()
    };
    let server = Server::start(script).await;

    server.send_logged_in(Security::StartTls).await.unwrap();

    let seen = server.seen();
    assert_eq!(seen.commands[..2], ["EHLO localhost", "STARTTLS"]);
    let auth = seen.encrypted_commands[1]
        .strip_prefix("AUTH PLAIN ")
        .unwrap();
    assert_eq!(decode_base64(auth), b"\0mailer\0secret password");
    assert_eq!(seen.encrypted_commands[0], "EHLO localhost");
    assert_eq!(seen.encrypted_commands.last().unwrap(), "QUIT");
    assert_ne!(seen.message, "");
}

#[tokio::test]
async fn tls_from_the_start_and_auth_login_deliver_too() {
    let script = Script {
        encryption: Encryption::Implicit,
        auth: "LOGIN",
        ..Script::default()
    };
    let server = Server::start(script).await;

    server.send_logged_in(Security::Tls).await.unwrap();

    let seen = server.seen();
    assert_eq!(seen.encrypted_commands[1], "AUTH LOGIN");
    assert_eq!(decode_base64(&seen.encrypted_commands[2]), b"mailer");
    assert_eq!(
        decode_base64(&seen.encrypted_commands[3]),
        b"secret password"
    );
    assert_eq!(seen.encrypted_commands[4], "MAIL FROM:<shop@example.com>");
}

#[tokio::test]
async fn a_recipient_refused_for_good_is_left_out_and_the_others_get_the_mail() {
    let script = Script {
        refused: &["bob@example.com"],
        refusal: "550 5.1.1 {address} unknown",
        ..Script::default()
    };
    let server = Server::start(script).await;

    let refused = server.send(Security::None, &mail()).await.unwrap();

    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].recipient(), &address("bob@example.com"));
    assert_eq!(refused[0].code(), 550);
    assert_eq!(refused[0].reply(), "550 5.1.1 [address] unknown");
    let seen = server.seen();
    assert!(seen.commands.contains(&"DATA".to_owned()));
    assert_ne!(seen.message, "");
}

#[tokio::test]
async fn a_mail_every_recipient_refused_is_not_sent() {
    let script = Script {
        refused: &["ada@example.com", "bob@example.com"],
        refusal: "550 5.1.1 {address} unknown",
        ..Script::default()
    };
    let server = Server::start(script).await;

    let refused = server.send(Security::None, &mail()).await.unwrap();

    assert_eq!(refused.len(), 2);
    let seen = server.seen();
    assert!(!seen.commands.contains(&"DATA".to_owned()));
    assert_eq!(seen.commands.last().unwrap(), "QUIT");
}

#[tokio::test]
async fn a_recipient_refused_for_now_is_returned_and_the_others_get_the_mail() {
    let script = Script {
        refused: &["bob@example.com"],
        refusal: "451 4.3.0 {address} try again later",
        ..Script::default()
    };
    let server = Server::start(script).await;

    let refused = server.send(Security::None, &mail()).await.unwrap();

    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].code(), 451);
    assert!(refused[0].is_deferred());
    assert_ne!(server.seen().message, "");
}

#[tokio::test]
async fn a_message_refused_for_good_fails_as_a_rule_and_one_refused_for_now_as_unavailable() {
    let refused_for_good = Server::start(Script {
        message_reply: "554 5.6.0 message refused",
        ..Script::default()
    })
    .await;
    let refused_for_now = Server::start(Script {
        message_reply: "421 4.4.2 closing, try again",
        ..Script::default()
    })
    .await;

    let permanent = refused_for_good
        .send(Security::None, &mail())
        .await
        .unwrap_err();
    let transient = refused_for_now
        .send(Security::None, &mail())
        .await
        .unwrap_err();

    assert!(
        matches!(&permanent, Error::Rule(message) if message.ends_with("the message with 554 5.6.0 message refused")),
        "{permanent}"
    );
    assert!(
        matches!(&transient, Error::Unavailable(message) if message.contains("with 421")),
        "{transient}"
    );
}

#[tokio::test]
async fn a_server_without_starttls_and_a_line_injected_before_tls_are_refused() {
    let without = Server::start(Script::default()).await;
    let injected = Server::start(Script {
        encryption: Encryption::StartTlsInjected,
        ..Script::default()
    })
    .await;

    let refused = without.send(Security::StartTls, &mail()).await.unwrap_err();
    let injection = injected
        .send(Security::StartTls, &mail())
        .await
        .unwrap_err();

    assert!(
        refused.to_string().contains("does not offer STARTTLS"),
        "{refused}"
    );
    assert!(
        injection.to_string().contains("before TLS began"),
        "{injection}"
    );
    assert_eq!(without.seen().message, "");
}

fn international_mail() -> Mail {
    Mail::new(
        address("çağrı@example.com"),
        "Your order",
        body("<p>Hi</p>"),
    )
    .unwrap()
    .sent_from(address("shop@example.com"))
}

#[tokio::test]
async fn an_address_beyond_ascii_is_sent_with_smtputf8_to_a_server_offering_it() {
    let server = Server::start(Script {
        smtputf8: true,
        ..Script::default()
    })
    .await;

    server
        .send(Security::None, &international_mail())
        .await
        .unwrap();

    let seen = server.seen();
    assert_eq!(
        seen.commands[1..3],
        [
            "MAIL FROM:<shop@example.com> SMTPUTF8",
            "RCPT TO:<çağrı@example.com>"
        ]
    );
    assert!(seen.message.contains("\r\nTo: çağrı@example.com\r\n"));
}

#[tokio::test]
async fn an_address_beyond_ascii_is_not_sent_to_a_server_without_smtputf8() {
    let server = Server::start(Script::default()).await;

    let error = server
        .send(Security::None, &international_mail())
        .await
        .unwrap_err();

    assert!(
        matches!(&error, Error::Rule(message) if message.contains("does not offer SMTPUTF8")),
        "{error}"
    );
    let seen = server.seen();
    assert!(
        !seen
            .commands
            .iter()
            .any(|command| command.starts_with("MAIL")),
        "{:?}",
        seen.commands
    );
}

#[tokio::test]
async fn an_ascii_mail_asks_no_smtputf8_of_a_server_offering_it() {
    let server = Server::start(Script {
        smtputf8: true,
        ..Script::default()
    })
    .await;

    server.send(Security::None, &mail()).await.unwrap();

    assert_eq!(server.seen().commands[1], "MAIL FROM:<shop@example.com>");
}
