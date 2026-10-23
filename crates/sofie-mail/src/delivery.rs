use std::sync::Arc;
use std::time::{Duration, Instant};

use sofie_auth::SigningKeys;

use sofie_core::{Error, Result};
use sofie_db::worker::JobContext;
use sofie_db::{
    Context, Db, IdempotencyKey, Jobs, Keyed, Queued, Reach, Read, Reply, Resolve, Work,
};
use tracing::Instrument;
use tracing::field::Empty;

use crate::suppression::{self, Reason, Scope, Suppression};
use crate::transport::{Envelope, Refused, Transport};
use crate::{Mail, Setup};

pub(crate) const JOB: &str = "sofie_mail.deliver";

#[cfg(feature = "metrics")]
static SEND_DURATION: sofie_telemetry::metrics::Instrument = sofie_telemetry::metrics::Instrument {
    name: "sofie.mail.duration",
    description: "How long sending a mail took, by transport and outcome (ok, refused or error).",
    kind: sofie_telemetry::metrics::Kind::Duration,
    labels: &["mail.transport", "outcome"],
};

pub(crate) struct Delivery {
    db: Db,
    setup: Setup,
    mail: Mail,
    work: Work,
    keys: Option<Arc<SigningKeys>>,
}

impl Resolve<JobContext> for Delivery {
    async fn resolve(context: &mut JobContext) -> Result<Self> {
        let setup = crate::installed(context).await?;
        let key = IdempotencyKey::resolve(context).await?;
        let mail = serde_json::from_value(context.input()?)
            .map_err(|error| Error::internal("reading a queued mail", error))?;
        Ok(Self {
            db: context.db().clone(),
            setup,
            mail,
            work: Work::new(key.as_str().to_owned(), JOB.to_owned()),
            keys: context.signing_keys().ok(),
        })
    }
}

/// At least once: marked sent right after the transport accepted it, so a retry sends nothing
/// again, unless marking failed or the server's answer came too late; the `Message-ID` stays.
pub(crate) async fn deliver(delivery: Delivery) -> Result<()> {
    let keyed = Keyed::new(&delivery.db, &delivery.work);
    if was_sent(&keyed).await? {
        return Ok(());
    }
    let key = delivery.work.key();
    let key = key.as_str();
    let Some(mail) = addressed(&delivery).await? else {
        tracing::info!(
            mail.job = key,
            "every recipient is suppressed; nothing was sent"
        );
        return finish(&keyed).await;
    };
    let envelope = Envelope {
        mail: &mail,
        from: mail.from().unwrap_or(&delivery.setup.from),
        key,
        date: delivery.db.clock().now().timestamp(),
    };
    let (deferred, refused): (Vec<Refused>, Vec<Refused>) =
        traced(delivery.setup.transport.as_ref(), &envelope)
            .await?
            .into_iter()
            .partition(Refused::is_deferred);
    if !deferred.is_empty() && deferred.len() + refused.len() == mail.to().len() {
        suppress_bounced(&delivery.db, &refused).await?;
        return Err(Error::Unavailable(format!(
            "the mail server took no recipient, some for now: {}",
            replies(&deferred)
        )));
    }
    let mut again = later(&delivery, &deferred)?;
    keyed.commit(&mut again, Reach::All).await?;
    keyed.record(&Reply::Empty).await?;
    suppress_bounced(&delivery.db, &refused).await?;
    refusal(key, &refused, mail.to().len())
}

fn later(delivery: &Delivery, deferred: &[Refused]) -> Result<Option<Queued>> {
    if deferred.is_empty() {
        return Ok(None);
    }
    let to = deferred.iter().map(|refused| refused.recipient().clone());
    let mail = delivery.mail.clone().sent_to(to.collect(), None);
    let now = delivery.db.clock().now();
    Jobs::queue_named_at(JOB, &mail, now).map(Some)
}

fn replies(refused: &[Refused]) -> String {
    let replies: Vec<&str> = refused.iter().map(Refused::reply).collect();
    replies.join("; ")
}

async fn finish(keyed: &Keyed<'_>) -> Result<()> {
    keyed.commit(&mut (), Reach::All).await?;
    keyed.record(&Reply::Empty).await
}

/// Another delivery may suppress the same address between the read and the commit;
/// the second try reads it and leaves it out.
async fn suppress_bounced(db: &Db, refused: &[Refused]) -> Result<()> {
    let mut first = bounced(&db.read(), refused).await?;
    let Err(error) = db.commit(&mut first).await else {
        return Ok(());
    };
    tracing::warn!(error = %error, "suppressing bounced addresses failed; reading them again");
    let mut second = bounced(&db.read(), refused).await?;
    db.commit(&mut second).await
}

/// Suppressed as a provider's report of a hard bounce would be, so no later mail tries the address.
async fn bounced(read: &Read, refused: &[Refused]) -> Result<Vec<Suppression>> {
    let dead: Vec<&str> = refused
        .iter()
        .filter(|refusal| refusal.names_a_dead_address())
        .map(|refusal| refusal.recipient().email().as_str())
        .collect();
    suppression::suppress_each(read, &dead, &Scope::All, Reason::Bounce).await
}

/// A mail no recipient took fails as `Rule`, which the worker buries instead of retrying.
fn refusal(key: &str, refused: &[Refused], recipients: usize) -> Result<()> {
    if refused.is_empty() {
        return Ok(());
    }
    let replies = replies(refused);
    if refused.len() == recipients {
        return Err(Error::Rule(format!(
            "the mail server refused every recipient: {replies}"
        )));
    }
    tracing::warn!(
        mail.job = key,
        mail.refused = refused.len(),
        mail.replies = replies,
        "the mail server refused some recipients; the others got the mail"
    );
    Ok(())
}

async fn addressed(delivery: &Delivery) -> Result<Option<Mail>> {
    let mail = &delivery.mail;
    let kept = suppression::reachable(&delivery.db.read(), mail).await?;
    if kept.is_empty() {
        return Ok(None);
    }
    let unsubscribe = mail
        .list()
        .map(|list| unsubscribe_link(delivery, &kept[0], list))
        .transpose()?;
    Ok(Some(mail.clone().sent_to(kept, unsubscribe)))
}

fn unsubscribe_link(delivery: &Delivery, recipient: &crate::Address, list: &str) -> Result<String> {
    let keys = delivery.keys.as_ref().ok_or_else(|| {
        Error::internal_message(
            "bulk mail seals its unsubscribe link with the app's keys; turn on sofie's `signed-links`",
        )
    })?;
    let base =
        delivery.setup.base_url.as_deref().ok_or_else(|| {
            crate::settings::missing_base_url("a bulk mail links back to the app")
        })?;
    let token = crate::unsubscribe::token(keys, recipient.email().as_str(), list)?;
    Ok(format!("{base}{}?token={token}", crate::unsubscribe::PATH))
}

/// A key is claimed only after a send, so one still pending is a send whose record was cut short.
async fn was_sent(keyed: &Keyed<'_>) -> Result<bool> {
    match keyed.replay().await {
        Ok(answered) => Ok(answered.is_some()),
        Err(Error::Rule(_)) => Ok(true),
        Err(error) => Err(error),
    }
}

async fn traced(transport: &dyn Transport, envelope: &Envelope<'_>) -> Result<Vec<Refused>> {
    let span = tracing::info_span!(
        "mail",
        otel.name = "mail.send",
        otel.kind = "client",
        mail.transport = transport.name(),
        mail.recipients = envelope.mail.to().len(),
        server.address = Empty,
        server.port = Empty,
        outcome = Empty,
    );
    if let Some((host, port)) = transport.server() {
        span.record("server.address", host);
        span.record("server.port", port);
    }
    let started = Instant::now();
    let result = transport.send(envelope).instrument(span.clone()).await;
    let outcome = match &result {
        Ok(refused) if refused.len() == envelope.mail.to().len() => "refused",
        Ok(_) => "ok",
        Err(_) => "error",
    };
    span.record("outcome", outcome);
    if let Err(error) = &result {
        span.in_scope(|| tracing::warn!(error = %error, "sending a mail failed"));
    }
    record(transport.name(), outcome, started.elapsed());
    result
}

fn record(transport: &'static str, outcome: &'static str, elapsed: Duration) {
    #[cfg(feature = "metrics")]
    {
        let labels = [transport, outcome].map(sofie_telemetry::metrics::Label::from);
        sofie_telemetry::metrics::record_duration(&SEND_DURATION, &labels, elapsed);
    }
    #[cfg(not(feature = "metrics"))]
    let _ = (transport, outcome, elapsed);
}
