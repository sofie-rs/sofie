use std::net::IpAddr;

use bytes::Bytes;
use http::StatusCode;
use sofie_auth::digest;
use sofie_core::Result;
use sofie_db::{Context, Db, Keyed, KeyedWork, Reply, Run, Work};

use crate::admission::{Admission, Visit};
use crate::answer::{self, Response};
use crate::throttle::Requester;
use crate::{Area, RequestContext};

const IDEMPOTENCY_KEY: &str = "idempotency-key";
/// What vendors accept as their own idempotency key, and enough for a UUID.
const MAX_KEY_LENGTH: usize = 255;
const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_hours(1);
const KEY_REQUIRED: &str =
    "This request needs an Idempotency-Key header of 1 to 255 visible characters.";

pub(crate) async fn sweep_expired_keys(db: Db) {
    loop {
        tokio::time::sleep(SWEEP_EVERY).await;
        if let Err(error) = sofie_db::sweep_expired_keys(&db).await {
            tracing::warn!(error.kind = "idempotency", error = %error, "sweeping expired idempotency keys failed");
        }
    }
}

impl Admission {
    /// Keyed within the route and the principal, so no one replays another's answer.
    pub(crate) fn work(
        &self,
        visit: Visit<'_>,
        pattern: &str,
        body: &Bytes,
    ) -> Result<Option<Work>, Box<Response>> {
        #[cfg(feature = "webhooks")]
        if let Some(verify) = &self.rules.verify {
            return crate::webhook::delivered_work(verify.as_ref(), visit, pattern, body).map(Some);
        }
        if !self.rules.idempotent {
            return Ok(None);
        }
        requested_work(visit, pattern, body).map(Some)
    }
}

fn requested_work(visit: Visit<'_>, pattern: &str, body: &Bytes) -> Result<Work, Box<Response>> {
    let request = visit.asked.request;
    let key = request
        .headers
        .get(IDEMPOTENCY_KEY)
        .and_then(|key| key.to_str().ok())
        .filter(|key| (1..=MAX_KEY_LENGTH).contains(&key.len()))
        .ok_or_else(|| {
            Box::new(answer::refusal(
                StatusCode::BAD_REQUEST,
                KEY_REQUIRED,
                visit.asked.caller,
            ))
        })?;
    let key = digest(&[
        b"request",
        request.method.as_str().as_bytes(),
        pattern.as_bytes(),
        &requester_bytes(visit),
        key.as_bytes(),
    ]);
    // The path, not the pattern: one key sent for another record is another request.
    let path = request.uri.path();
    let query = request.uri.query().unwrap_or_default();
    Ok(Work::new(
        key,
        digest(&[path.as_bytes(), query.as_bytes(), body]),
    ))
}

/// An anonymous caller by its address, so it never replays another anonymous caller's answer.
fn requester_bytes(visit: Visit<'_>) -> Vec<u8> {
    match Requester::of(visit.principal, visit.asked.client) {
        Requester::Principal(principal) => [b"principal:", &principal.as_bytes()[..]].concat(),
        Requester::Address(IpAddr::V4(address)) => [b"address:", &address.octets()[..]].concat(),
        Requester::Address(IpAddr::V6(address)) => [b"address:", &address.octets()[..]].concat(),
    }
}

impl<A: Area> KeyedWork for RequestContext<A> {
    fn work(&self) -> Option<&Work> {
        self.matched.work.as_ref()
    }
}

/// A key claimed by a concurrent request gets its answer instead of writing twice.
pub(crate) async fn answer_once<A: Area>(
    run: &Run<RequestContext<A>>,
    mut context: RequestContext<A>,
    work: &Work,
) -> Result<Reply> {
    let db = context.db().clone();
    let keyed = Keyed::new(&db, work);
    if let Some(reply) = keyed.replay().await? {
        return Ok(reply);
    }
    let mut outcome = run(&mut context).await?;
    context.matched.deadline.lift();
    if let Some(reply) = keyed.commit(&mut *outcome, context.reach()).await? {
        return Ok(reply);
    }
    context.session().committed();
    let reply = outcome.into_reply()?;
    keyed.record(&reply).await?;
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use sofie_core::Timestamp;

    use super::*;
    use crate::caller::Caller;
    use crate::deadline::Deadline;

    fn work(path: &str, client: &str, principal: Option<sofie_core::Uuid>) -> Work {
        let (request, ()) = http::Request::post(path)
            .header(IDEMPOTENCY_KEY, "same-key")
            .body(())
            .unwrap()
            .into_parts();
        let deadline = Deadline::default();
        let visit = Visit {
            asked: crate::pipeline::Asked {
                request: &request,
                caller: Caller::Json,
                client: client.parse().unwrap(),
                deadline: &deadline,
            },
            principal,
            now: Timestamp::UNIX_EPOCH,
            #[cfg(feature = "signed-links")]
            keys: &sofie_auth::SigningKeys::generate().unwrap(),
        };
        let body = Bytes::new();
        requested_work(visit, "/orders/{order}/cancel", &body).unwrap_or_else(|_| panic!("keyed"))
    }

    fn keyed(client: &str, principal: Option<sofie_core::Uuid>) -> String {
        work("/orders/1/cancel", client, principal)
            .key()
            .as_str()
            .to_owned()
    }

    #[test]
    fn one_key_sent_for_another_record_is_another_request() {
        let principal = Some(sofie_core::Uuid::now_v7().unwrap());

        let first = work("/orders/1/cancel", "192.0.2.1", principal);
        let second = work("/orders/2/cancel", "192.0.2.1", principal);

        assert_eq!(first.key().as_str(), second.key().as_str());
        assert_ne!(format!("{first:?}"), format!("{second:?}"));
    }

    #[test]
    fn anonymous_callers_never_share_a_key_and_a_principal_keeps_its_own() {
        let principal = Some(sofie_core::Uuid::now_v7().unwrap());

        assert_ne!(keyed("192.0.2.1", None), keyed("192.0.2.2", None));
        assert_eq!(keyed("192.0.2.1", principal), keyed("192.0.2.2", principal));
    }
}
