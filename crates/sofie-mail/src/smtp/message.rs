//! The message as SMTP's DATA carries it: RFC 5322 headers and a MIME
//! multipart/alternative body, all 7-bit, so any server relays it unchanged, unless an
//! address is beyond ASCII, which only a server offering SMTPUTF8 is sent.

use jiff::fmt::rfc2822::DateTimePrinter;
use sofie_core::{Error, Result};

use crate::Address;
use crate::transport::Envelope;
use sofie_core::encoding::base64::{STANDARD, encode_padded, push_padded};

const LINE_BYTES: usize = 57;
/// An encoded word stays within 75 characters: `=?UTF-8?B?` and `?=` around 60 of base64.
const ENCODED_WORD_BYTES: usize = 45;

pub(crate) fn render(envelope: &Envelope<'_>) -> Result<String> {
    let mail = envelope.mail;
    // In UTC as `+0000`; RFC 5322 reads `-0000` as a zone left unknown.
    let date = DateTimePrinter::new()
        .zoned_to_string(&envelope.date.to_zoned(jiff::tz::TimeZone::UTC))
        .map_err(|error| Error::internal("writing a mail's date", error))?;
    // Base64 never holds a `-`, so no line of a part can equal the boundary.
    let boundary = format!("sofie-mail-{}", envelope.key);
    let recipients: Vec<String> = mail.to().iter().map(address).collect();
    let mut out = String::new();
    header(&mut out, "Date", &date);
    header(&mut out, "From", &address(envelope.from));
    header(&mut out, "To", &recipients.join(",\r\n "));
    if let Some(reply_to) = mail.reply_to() {
        header(&mut out, "Reply-To", &address(reply_to));
    }
    header(&mut out, "Subject", &text(mail.subject()));
    header(
        &mut out,
        "Message-ID",
        &format!("<{}>", envelope.message_id()),
    );
    if let Some(link) = mail.unsubscribe() {
        header(&mut out, "List-Unsubscribe", &format!("<{link}>"));
        header(
            &mut out,
            "List-Unsubscribe-Post",
            "List-Unsubscribe=One-Click",
        );
    }
    header(&mut out, "MIME-Version", "1.0");
    let content_type = format!("multipart/alternative; boundary=\"{boundary}\"");
    header(&mut out, "Content-Type", &content_type);
    out.push_str("\r\n");
    part(&mut out, &boundary, "text/plain", &[mail.body().text()]);
    part(
        &mut out,
        &boundary,
        "text/html",
        &[DOCUMENT_START, mail.body().html(), DOCUMENT_END],
    );
    out.push_str("--");
    out.push_str(&boundary);
    out.push_str("--\r\n");
    Ok(out)
}

fn header(out: &mut String, name: &str, value: &str) {
    out.push_str(name);
    out.push_str(": ");
    out.push_str(value);
    out.push_str("\r\n");
}

const DOCUMENT_START: &str = "<!doctype html>\r\n<html><head><meta charset=\"utf-8\"></head><body>";
const DOCUMENT_END: &str = "</body></html>\r\n";

/// RFC 2045 encodes text with CRLF line breaks, so a bare CR or LF is made one first.
fn part(out: &mut String, boundary: &str, media_type: &str, pieces: &[&str]) {
    out.push_str("--");
    out.push_str(boundary);
    out.push_str("\r\nContent-Type: ");
    out.push_str(media_type);
    out.push_str("; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n");
    let length: usize = pieces.iter().map(|piece| piece.len()).sum();
    let mut text = Vec::with_capacity(length + length / 32);
    for piece in pieces {
        push_crlf(&mut text, piece.as_bytes());
    }
    out.reserve(text.len().div_ceil(LINE_BYTES) * 78);
    for line in text.chunks(LINE_BYTES) {
        push_padded(out, &STANDARD, line);
        out.push_str("\r\n");
    }
}

fn push_crlf(text: &mut Vec<u8>, bytes: &[u8]) {
    let mut previous = 0;
    for &byte in bytes {
        match byte {
            b'\n' if previous == b'\r' => {}
            b'\r' | b'\n' => text.extend_from_slice(b"\r\n"),
            _ => text.push(byte),
        }
        previous = byte;
    }
}

/// The address was checked when it was made, so it needs no escaping.
fn address(address: &Address) -> String {
    let email = address.email().as_str();
    match address.name() {
        None => email.to_owned(),
        Some(name) if is_plain(name) => {
            let quoted = name.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{quoted}\" <{email}>")
        }
        Some(name) => format!("{} <{email}>", encoded_words(name)),
    }
}

fn text(value: &str) -> String {
    if is_plain(value) {
        return value.to_owned();
    }
    encoded_words(value)
}

/// ASCII holding `=?` is encoded too: a client would decode it as an encoded word, into
/// characters the display checks never saw.
fn is_plain(value: &str) -> bool {
    value.is_ascii() && !value.contains("=?")
}

fn encoded_words(value: &str) -> String {
    let mut words = Vec::new();
    let mut start = 0;
    let mut end = 0;
    for (index, character) in value.char_indices() {
        let next = index + character.len_utf8();
        if next - start > ENCODED_WORD_BYTES {
            words.push(encoded_word(&value[start..end]));
            start = end;
        }
        end = next;
    }
    words.push(encoded_word(&value[start..end]));
    words.join("\r\n ")
}

fn encoded_word(chunk: &str) -> String {
    format!("=?UTF-8?B?{}?=", encode_padded(&STANDARD, chunk.as_bytes()))
}

#[cfg(test)]
mod tests {
    use sofie_core::Timestamp;
    use sofie_values::Email;

    use super::*;
    use crate::{Body, Mail};

    fn address(email: &str) -> Address {
        Address::new(&Email::parse(email).unwrap()).unwrap()
    }

    #[test]
    fn a_bulk_mail_offers_one_click_unsubscribe() {
        let from = address("shop@example.com");
        let to = address("ada@example.com");
        let mail = Mail::new(to.clone(), "News", Body::rendered("<p>News</p>".to_owned()))
            .unwrap()
            .bulk("news")
            .unwrap()
            .sent_to(
                vec![to],
                Some("https://shop.example/mail/unsubscribe?token=t".to_owned()),
            );
        let envelope = Envelope {
            mail: &mail,
            from: &from,
            key: "0190c3e2-0000-7000-8000-000000000001",
            date: Timestamp::from_second(1_700_000_000).unwrap(),
        };

        let message = render(&envelope).unwrap();

        assert!(message.contains(
            "List-Unsubscribe: <https://shop.example/mail/unsubscribe?token=t>\r\n\
             List-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n"
        ));
    }

    #[test]
    fn a_mail_is_headers_and_two_base64_parts() {
        let from = address("shop@example.com").named("Shop \"Main\"").unwrap();
        let to = address("ada@example.com").named("Ada Lovelace").unwrap();
        let body = Body::rendered("<p>Hi Ada</p>".to_owned());
        let mail = Mail::new(to, "Your receipt", body)
            .unwrap()
            .also_to(address("bob@example.com"))
            .replying_to(address("help@example.com"));
        let envelope = Envelope {
            mail: &mail,
            from: &from,
            key: "0190c3e2-0000-7000-8000-000000000001",
            date: Timestamp::from_second(1_700_000_000).unwrap(),
        };

        let message = render(&envelope).unwrap();

        let (head, body) = message.split_once("\r\n\r\n").unwrap();
        assert_eq!(
            head,
            "Date: Tue, 14 Nov 2023 22:13:20 +0000\r\n\
             From: \"Shop \\\"Main\\\"\" <shop@example.com>\r\n\
             To: \"Ada Lovelace\" <ada@example.com>,\r\n bob@example.com\r\n\
             Reply-To: help@example.com\r\n\
             Subject: Your receipt\r\n\
             Message-ID: <0190c3e2-0000-7000-8000-000000000001@example.com>\r\n\
             MIME-Version: 1.0\r\n\
             Content-Type: multipart/alternative; \
             boundary=\"sofie-mail-0190c3e2-0000-7000-8000-000000000001\""
        );
        let boundary = "--sofie-mail-0190c3e2-0000-7000-8000-000000000001";
        assert!(body.starts_with(&format!(
            "{boundary}\r\nContent-Type: text/plain; charset=utf-8\r\n\
             Content-Transfer-Encoding: base64\r\n\r\n{}\r\n",
            encode_padded(&STANDARD, b"Hi Ada")
        )));
        assert!(body.ends_with(&format!("{boundary}--\r\n")));
        assert!(message.is_ascii());
        assert!(message.lines().all(|line| line.len() <= 998));
    }

    #[test]
    fn non_ascii_text_is_split_into_encoded_words_of_whole_characters() {
        let subject = "Siparişiniz hazır: çay, şeker ve üç kahve fincanı, hepsi yolda ğğğ";

        let encoded = text(subject);

        let words: Vec<&str> = encoded.split("\r\n ").collect();
        assert!(words.len() > 1);
        assert!(words.iter().all(|word| word.len() <= 75), "{encoded}");
        assert!(words.iter().all(|word| word.starts_with("=?UTF-8?B?")));
        assert_eq!(text("Plain"), "Plain");
    }

    #[test]
    fn ascii_spelling_an_encoded_word_is_encoded_itself() {
        let forged = "Order =?UTF-8?B?4oCu?= shipped";

        let encoded = text(forged);

        assert!(encoded.starts_with("=?UTF-8?B?"), "{encoded}");
        assert!(!encoded.contains("4oCu?="), "{encoded}");
        let named = address_named("=?UTF-8?B?YWRtaW5AYmFuay5jb20=?=");
        assert!(!named.starts_with('"'), "{named}");
    }

    #[test]
    fn text_is_encoded_with_crlf_line_breaks() {
        let mut out = String::new();

        part(&mut out, "b", "text/plain", &["one\ntwo\r\nthree\rfour"]);

        let encoded = out.split("\r\n\r\n").nth(1).unwrap().trim_end();
        assert_eq!(
            encoded,
            encode_padded(&STANDARD, b"one\r\ntwo\r\nthree\r\nfour")
        );
    }

    fn address_named(name: &str) -> String {
        super::address(&address("ada@example.com").named(name).unwrap())
    }
}
