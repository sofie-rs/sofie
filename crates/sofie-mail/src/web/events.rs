use ring::hmac;
use serde_json::Value;
use sofie_core::encoding::{base64, hex};
use sofie_core::{Error, Result};
use sofie_db::{Read, Resolve, Writes};
use sofie_http::{Delivery, Routes, Verify, Web};
use sofie_values::Secret;

use super::Suppressed;
use crate::suppression::{self, Reason, Scope};

/// Mailgun signs a timestamp; one older than this is refused as a replay.
const FRESH_SECONDS: i64 = 15 * 60;
const UNVERIFIED: &str = "the report could not be verified";

#[derive(Default)]
pub struct Events {
    /// The `user:password` Postmark's webhook URL carries as basic authentication.
    pub postmark: Option<Secret<String>>,
    pub mailgun: Option<Secret<String>>,
    /// For any other provider: an HMAC-SHA256 key over the body, sent as hex in
    /// `X-Sofie-Signature`, the body being `{"id", "event": "bounce"|"complaint", "email"}`.
    pub signed: Option<Secret<String>>,
}

impl Events {
    /// `SOFIE_MAIL_POSTMARK_WEBHOOK_AUTH`, `SOFIE_MAIL_MAILGUN_SIGNING_KEY` and
    /// `SOFIE_MAIL_WEBHOOK_SECRET`; a provider left unset gets no route.
    pub fn from_environment() -> Result<Self> {
        let read = |name| Ok::<_, Error>(sofie_core::environment_variable(name)?.map(Secret::new));
        Ok(Self {
            postmark: read("SOFIE_MAIL_POSTMARK_WEBHOOK_AUTH")?,
            mailgun: read("SOFIE_MAIL_MAILGUN_SIGNING_KEY")?,
            signed: read("SOFIE_MAIL_WEBHOOK_SECRET")?,
        })
    }

    pub(super) fn routes(&self, routes: &mut Routes<Web>) {
        if let Some(auth) = &self.postmark {
            routes
                .webhook("/mail/events/postmark", postmark)
                .verify(Postmark(sofie_auth::digest(&[auth.expose().as_bytes()])));
        }
        if let Some(key) = &self.mailgun {
            routes
                .webhook("/mail/events/mailgun", mailgun)
                .verify(Mailgun(hmac_key(key)));
        }
        if let Some(key) = &self.signed {
            routes
                .webhook("/mail/events/signed", signed)
                .verify(Signed(hmac_key(key)));
        }
    }
}

fn hmac_key(key: &Secret<String>) -> hmac::Key {
    hmac::Key::new(hmac::HMAC_SHA256, key.expose().as_bytes())
}

fn json(body: &[u8]) -> Result<Value> {
    serde_json::from_slice(body).map_err(|_| Error::BadRequest("the report is not JSON".to_owned()))
}

fn unverified() -> Error {
    Error::BadRequest(UNVERIFIED.to_owned())
}

/// Compared by digest, so the time taken tells nothing of the secret.
struct Postmark(String);

impl Verify for Postmark {
    fn verify(&self, delivery: &Delivery<'_>) -> Result<String> {
        let given = delivery
            .header("authorization")
            .and_then(|value| value.strip_prefix("Basic "))
            .and_then(|encoded| base64::decode(&base64::STANDARD, encoded))
            .ok_or_else(unverified)?;
        if sofie_auth::digest(&[&given]) != self.0 {
            return Err(unverified());
        }
        report_id(delivery.body(), "ID")
    }
}

struct Mailgun(hmac::Key);

impl Verify for Mailgun {
    fn verify(&self, delivery: &Delivery<'_>) -> Result<String> {
        let report = json(delivery.body())?;
        let signature = &report["signature"];
        let (Some(timestamp), Some(token), Some(signed)) = (
            signature["timestamp"].as_str(),
            signature["token"].as_str(),
            signature["signature"].as_str().and_then(hex::decode),
        ) else {
            return Err(unverified());
        };
        let fresh = timestamp
            .parse::<i64>()
            .is_ok_and(|at| (delivery.now().as_second() - at).abs() <= FRESH_SECONDS);
        let message = format!("{timestamp}{token}");
        if !fresh || hmac::verify(&self.0, message.as_bytes(), &signed).is_err() {
            return Err(unverified());
        }
        Ok(token.to_owned())
    }
}

struct Signed(hmac::Key);

impl Verify for Signed {
    fn verify(&self, delivery: &Delivery<'_>) -> Result<String> {
        let signed = delivery
            .header("x-sofie-signature")
            .and_then(hex::decode)
            .ok_or_else(unverified)?;
        hmac::verify(&self.0, delivery.body(), &signed).map_err(|_| unverified())?;
        report_id(delivery.body(), "id")
    }
}

fn report_id(body: &[u8], key: &str) -> Result<String> {
    match &json(body)?[key] {
        Value::String(id) if !id.is_empty() => Ok(id.clone()),
        Value::Number(id) => Ok(id.to_string()),
        _ => Err(Error::BadRequest(format!("the report has no `{key}`"))),
    }
}

pub(crate) struct Report(Value);

impl<C: Writes> Resolve<C> for Report {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(context.input().map(Self))
    }
}

async fn suppressed(
    read: &Read,
    address: Option<&str>,
    scope: Option<(Scope, Reason)>,
) -> Result<Suppressed> {
    let (Some(address), Some((scope, reason))) = (address, scope) else {
        return Ok(Suppressed(None));
    };
    suppression::suppress(read, address, scope, reason)
        .await
        .map(Suppressed)
}

/// A soft bounce or any other record type changes nothing.
async fn postmark(report: Report, read: Read) -> Result<Suppressed> {
    let report = report.0;
    let scope = match (report["RecordType"].as_str(), report["Type"].as_str()) {
        (Some("Bounce"), Some("HardBounce" | "BadEmailAddress")) => {
            Some((Scope::All, Reason::Bounce))
        }
        (Some("SpamComplaint"), _) => Some((Scope::Bulk, Reason::Complaint)),
        _ => None,
    };
    suppressed(&read, report["Email"].as_str(), scope).await
}

async fn mailgun(report: Report, read: Read) -> Result<Suppressed> {
    let event = &report.0["event-data"];
    let scope = match (event["event"].as_str(), event["severity"].as_str()) {
        (Some("failed"), Some("permanent")) => Some((Scope::All, Reason::Bounce)),
        (Some("complained"), _) => Some((Scope::Bulk, Reason::Complaint)),
        _ => None,
    };
    suppressed(&read, event["recipient"].as_str(), scope).await
}

async fn signed(report: Report, read: Read) -> Result<Suppressed> {
    let report = report.0;
    let scope = match report["event"].as_str() {
        Some("bounce") => Some((Scope::All, Reason::Bounce)),
        Some("complaint") => Some((Scope::Bulk, Reason::Complaint)),
        _ => None,
    };
    suppressed(&read, report["email"].as_str(), scope).await
}
