use std::sync::{Arc, Mutex};

use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;
use sofie_core::{Error, Result, Timestamp, unpoisoned};
use sofie_db::{BoxFuture, Db};
use sofie_values::Email;

use crate::smtp::Smtp;
pub use crate::transport::Refused;
use crate::transport::{Envelope, Transport};
use crate::{Address, Body, Mail, Setup, SmtpSettings};

pub const FROM: &str = "app@example.com";
pub const BASE_URL: &str = "http://localhost";

#[derive(Clone, Default)]
pub struct Outbox {
    sent: Arc<Mutex<Vec<Mail>>>,
    refusals: Arc<Mutex<Vec<(String, u16)>>>,
}

impl Outbox {
    #[must_use]
    pub fn mails(&self) -> Vec<Mail> {
        unpoisoned::lock(&self.sent).clone()
    }

    /// Later sends refuse `email` at `RCPT TO` as a mail server would answer
    /// with `code`: a 4xx for now, a 5xx for good.
    pub fn refuse(&self, email: &str, code: u16) {
        unpoisoned::lock(&self.refusals).push((email.to_owned(), code));
    }

    fn refusal(&self, recipient: &Address) -> Option<u16> {
        let refusals = unpoisoned::lock(&self.refusals);
        refusals
            .iter()
            .find(|(email, _)| email == recipient.email().as_str())
            .map(|(_, code)| *code)
    }
}

struct Fake(Outbox);

impl Transport for Fake {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn send<'a>(&'a self, envelope: &'a Envelope<'a>) -> BoxFuture<'a, Result<Vec<Refused>>> {
        Box::pin(std::future::ready(Ok(self.take(envelope))))
    }
}

impl Fake {
    fn take(&self, envelope: &Envelope<'_>) -> Vec<Refused> {
        let mail = envelope.mail;
        let mut accepted = Vec::new();
        let mut refused = Vec::new();
        for recipient in mail.to() {
            match self.0.refusal(recipient) {
                None => accepted.push(recipient.clone()),
                Some(code) => {
                    refused.push(Refused::new(
                        recipient.clone(),
                        code,
                        format!("{code} refused"),
                    ));
                }
            }
        }
        if !accepted.is_empty() {
            let unsubscribe = mail.unsubscribe().map(str::to_owned);
            let sent = mail.clone().sent_to(accepted, unsubscribe);
            unpoisoned::lock(&self.0.sent).push(sent.with_default_sender(envelope.from));
        }
        refused
    }
}

#[must_use]
pub fn fake(db: &Db) -> Outbox {
    installed(db, Some(BASE_URL))
}

/// As an app without `SOFIE_MAIL_BASE_URL` or `SOFIE_BASE_URL`, whose mails cannot link back to it.
#[must_use]
pub fn fake_without_base_url(db: &Db) -> Outbox {
    installed(db, None)
}

fn installed(db: &Db, base_url: Option<&str>) -> Outbox {
    let outbox = Outbox::default();
    let from = Email::parse(FROM).and_then(|email| Address::new(&email));
    let setup = Setup {
        from: from.expect("the fake's sender is a plain address"),
        base_url: base_url.map(str::to_owned),
        transport: Arc::new(Fake(outbox.clone())),
    };
    crate::install(db, &setup);
    outbox
}

#[must_use]
pub fn body(html: &str) -> Body {
    Body::rendered(html.to_owned())
}

pub struct TestSmtp(Smtp);

pub fn smtp(settings: SmtpSettings, root: &[u8]) -> Result<TestSmtp> {
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(root.to_vec()))
        .map_err(|error| Error::internal("trusting the test's root certificate", error))?;
    Ok(TestSmtp(Smtp::trusting(
        settings,
        "localhost".to_owned(),
        roots,
    )))
}

impl TestSmtp {
    /// `key` stands for the job's idempotency key, which names the message.
    pub async fn send(&self, mail: &Mail, key: &str, date: Timestamp) -> Result<Vec<Refused>> {
        let from = mail
            .from()
            .ok_or_else(|| Error::internal_message("a test's mail names its sender"))?;
        let envelope = Envelope {
            mail,
            from,
            key,
            date,
        };
        self.0.send(&envelope).await
    }
}
