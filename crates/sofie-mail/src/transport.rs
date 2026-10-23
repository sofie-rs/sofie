use std::fmt::Write as _;
use std::path::PathBuf;

use sofie_core::{Error, Result, Timestamp};
use sofie_db::BoxFuture;

use crate::{Address, Mail};

pub(crate) trait Transport: Send + Sync {
    fn name(&self) -> &'static str;

    fn server(&self) -> Option<(&str, u16)> {
        None
    }

    /// The recipients refused, for good or for now; the mail went to the others, if any.
    fn send<'a>(&'a self, envelope: &'a Envelope<'a>) -> BoxFuture<'a, Result<Vec<Refused>>>;
}

/// A recipient the mail server refused: for good with a 5xx reply, for now with a 4xx.
#[derive(Clone, Debug)]
pub struct Refused {
    recipient: Address,
    code: u16,
    /// The code and the server's words, any address in them left out.
    reply: String,
}

impl Refused {
    pub(crate) fn new(recipient: Address, code: u16, reply: String) -> Self {
        Self {
            recipient,
            code,
            reply,
        }
    }

    #[must_use]
    pub fn is_deferred(&self) -> bool {
        self.code < 500
    }

    #[must_use]
    pub fn recipient(&self) -> &Address {
        &self.recipient
    }

    #[must_use]
    pub fn code(&self) -> u16 {
        self.code
    }

    #[must_use]
    pub fn reply(&self) -> &str {
        &self.reply
    }

    /// 550, 551 and 553 say the mailbox is not there, unless an enhanced status, wherever the
    /// reply puts it, names something else: a policy (5.7.x, as when the server relays no mail
    /// for us), an address beyond ASCII (5.6.7, RFC 6531) or our own sender (5.1.7, 5.1.8).
    pub(crate) fn names_a_dead_address(&self) -> bool {
        let names_the_mailbox = self
            .reply
            .split_whitespace()
            .skip(1)
            .map(|word| word.trim_matches(|character: char| !character.is_ascii_digit()))
            .find(|word| is_enhanced_status(word))
            .is_none_or(|status| DEAD_MAILBOX.contains(&status));
        matches!(self.code(), 550 | 551 | 553) && names_the_mailbox
    }
}

const DEAD_MAILBOX: &[&str] = &["5.1.1", "5.1.2", "5.1.3", "5.1.6", "5.1.10"];

fn is_enhanced_status(word: &str) -> bool {
    let mut parts = word.split('.');
    parts.next() == Some("5")
        && parts.clone().count() == 2
        && parts.all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

pub(crate) struct Envelope<'a> {
    pub(crate) mail: &'a Mail,
    pub(crate) from: &'a Address,
    /// The job's idempotency key, so every try of one job sends the same `Message-ID`.
    pub(crate) key: &'a str,
    pub(crate) date: Timestamp,
}

impl Envelope<'_> {
    pub(crate) fn message_id(&self) -> String {
        format!("{}@{}", self.key, self.from.domain())
    }
}

/// For development: the mail, links included, is logged instead of sent.
pub(crate) struct Logged {
    pub(crate) preview: Option<PathBuf>,
}

impl Transport for Logged {
    fn name(&self) -> &'static str {
        "log"
    }

    fn send<'a>(&'a self, envelope: &'a Envelope<'a>) -> BoxFuture<'a, Result<Vec<Refused>>> {
        let mail = envelope.mail;
        tracing::info!(
            mail.from = ?envelope.from,
            mail.to = ?mail.to(),
            mail.subject = mail.subject(),
            mail.text = mail.body().text(),
            "mail written to the log, not sent"
        );
        let Some(folder) = self.preview.clone() else {
            return Box::pin(std::future::ready(Ok(Vec::new())));
        };
        let file = folder.join(format!(
            "{}-{}.html",
            envelope.date.as_second(),
            envelope.key
        ));
        let page = preview(envelope);
        Box::pin(async move {
            let written = file.clone();
            tokio::task::spawn_blocking(move || {
                std::fs::create_dir_all(&folder)?;
                std::fs::write(&written, page)
            })
            .await
            .map_err(|error| Error::internal("writing a mail preview", error))?
            .map_err(|error| Error::internal(format!("writing {}", file.display()), error))?;
            tracing::info!(mail.preview = %file.display(), "mail preview written");
            Ok(Vec::new())
        })
    }
}

fn preview(envelope: &Envelope<'_>) -> String {
    let mail = envelope.mail;
    let recipients: Vec<&str> = mail.to().iter().map(|to| to.email().as_str()).collect();
    let mut rows = vec![
        ("From", envelope.from.email().as_str().to_owned()),
        ("To", recipients.join(", ")),
        ("Subject", mail.subject().to_owned()),
    ];
    if let Some(link) = mail.unsubscribe() {
        rows.push(("List-Unsubscribe", link.to_owned()));
    }
    let mut page = String::from("<!doctype html><meta charset=\"utf-8\"><table>");
    for (name, value) in rows {
        let _ = write!(page, "<tr><th>{name}</th><td>");
        sofie_engine::push_escaped(&mut page, &value);
        page.push_str("</td></tr>");
    }
    page.push_str("</table><hr>");
    page.push_str(mail.body().html());
    page
}

#[cfg(test)]
mod tests {
    use sofie_values::Email;

    use super::*;

    fn refused(code: u16, reply: &str) -> Refused {
        let ada = Address::new(&Email::parse("ada@example.com").unwrap()).unwrap();
        Refused::new(ada, code, reply.to_owned())
    }

    #[test]
    fn only_a_missing_mailbox_names_a_dead_address() {
        assert!(refused(550, "550 5.1.1 [address] unknown").names_a_dead_address());
        assert!(refused(553, "553 mailbox name not allowed").names_a_dead_address());
        assert!(!refused(550, "550 5.7.1 relaying denied").names_a_dead_address());
        assert!(
            !refused(553, "553 5.6.7 non-ASCII addresses not permitted").names_a_dead_address()
        );
        assert!(!refused(552, "552 5.2.2 mailbox full").names_a_dead_address());
        assert!(
            !refused(
                553,
                "553 sorry, that domain isn't in my list of allowed rcpthosts (#5.7.1)"
            )
            .names_a_dead_address()
        );
        assert!(
            !refused(550, "550 5.1.8 [address]: Sender address rejected").names_a_dead_address()
        );
        assert!(refused(550, "550 5.1.10 null MX").names_a_dead_address());
    }
}
