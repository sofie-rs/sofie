//! Web push: browsers' subscriptions, and one job per subscription and notification,
//! so a failing push service retries only its own.

mod encrypt;
mod vapid;

use serde::{Deserialize, Serialize};
use serde_json::json;
use sofie_core::encoding::base64::{URL_SAFE, decode};
use sofie_core::{Clock, Error, Result, SqlValue, Timestamp, Uuid};
use sofie_db::worker::JobContext;
use sofie_db::{
    Aggregate, Dep, Dialect, Id, Input, Outcome, Plan, Read, Reply, Resolve, Select, Step, Writes,
};
use sofie_http::{Area, Principal};

pub use vapid::VapidKeys;

use crate::store::{PUSH_GONE_AT, PUSH_PRINCIPAL, PushSubscription};
use crate::{Notice, Notify, Setup};

pub(crate) const JOB: &str = "sofie_notify.push";
const MAXIMUM_ENDPOINT: usize = 2048;
const TIME_TO_LIVE: &str = "86400";
const MAXIMUM_PUSHED_BODY: usize = 500;
/// Each live subscription is a push job per notification; past this the oldest is let go.
const MAXIMUM_BROWSERS: i64 = 20;

/// What a browser's `PushManager.subscribe` answered, as `subscription.toJSON()` sends it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PushRegistration {
    endpoint: String,
    #[serde(rename = "expirationTime")]
    _expiration_time: Option<serde_json::Value>,
    keys: RegistrationKeys,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationKeys {
    p256dh: String,
    auth: String,
}

impl Input for PushRegistration {
    fn from_json(value: serde_json::Value) -> Result<Self> {
        let registration: Self = serde_json::from_value(value)
            .map_err(|_| Error::invalid("endpoint", "is not a push subscription"))?;
        let endpoint_ok = registration.endpoint.starts_with("https://")
            && registration.endpoint.len() <= MAXIMUM_ENDPOINT
            && !registration
                .endpoint
                .contains(|character: char| character.is_whitespace() || character.is_control());
        if !endpoint_ok {
            return Err(Error::invalid("endpoint", "must be an https URL"));
        }
        let browser_key = decode(&URL_SAFE, &registration.keys.p256dh);
        if !browser_key.is_some_and(|key| key.len() == 65 && key[0] == 4) {
            return Err(Error::invalid("keys.p256dh", "must be a P-256 public key"));
        }
        if decode(&URL_SAFE, &registration.keys.auth).is_none_or(|auth| auth.len() != 16) {
            return Err(Error::invalid("keys.auth", "must be a 16-byte secret"));
        }
        Ok(registration)
    }
}

impl<C: Writes> Resolve<C> for PushRegistration {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(context.input().and_then(Self::from_json))
    }
}

/// A route's handler: `routes.post("/push", sofie::notify::subscribe_push::<Member>)`.
/// A browser subscribed before for another principal now pushes to this one, if it shows the
/// same secret: an endpoint alone does not take a live subscription over (`Conflict`).
pub async fn subscribe_push<A: Area>(
    me: Principal<A>,
    registration: PushRegistration,
    clock: Clock,
) -> Result<Subscribed> {
    Ok(Subscribed {
        id: Uuid::now_v7()?,
        principal: me.id().uuid(),
        registration,
        now: clock.now().timestamp(),
    })
}

/// What a browser sends to stop, `{"endpoint": …}`, as `subscription.unsubscribe()` leaves it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PushEndpoint {
    endpoint: String,
}

impl Input for PushEndpoint {
    fn from_json(value: serde_json::Value) -> Result<Self> {
        let stopping: Self = serde_json::from_value(value)
            .map_err(|_| Error::invalid("endpoint", "is not a push endpoint"))?;
        if stopping.endpoint.len() > MAXIMUM_ENDPOINT {
            return Err(Error::invalid("endpoint", "is not a push endpoint"));
        }
        Ok(stopping)
    }
}

impl<C: Writes> Resolve<C> for PushEndpoint {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(context.input().and_then(Self::from_json))
    }
}

/// A route's handler: the browser stops getting this principal's pushes, as on signing out.
/// Another principal's subscription is left as it is, answered alike.
pub async fn unsubscribe_push<A: Area>(
    me: Principal<A>,
    stopping: PushEndpoint,
    clock: Clock,
) -> Result<Unsubscribed> {
    Ok(Unsubscribed {
        principal: me.id().uuid(),
        endpoint: stopping.endpoint,
        now: clock.now().timestamp(),
    })
}

pub struct Unsubscribed {
    principal: Uuid,
    endpoint: String,
    now: Timestamp,
}

impl Outcome for Unsubscribed {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        let dialect = plan.dialect();
        let sql = format!(
            "UPDATE {} SET gone_at = {}, version = version + 1 \
             WHERE endpoint = {} AND principal = {} AND gone_at IS NULL",
            push_table(dialect),
            dialect.placeholder(1),
            dialect.placeholder(2),
            dialect.placeholder(3),
        );
        let params = vec![
            SqlValue::Timestamp(self.now),
            SqlValue::Text(self.endpoint.clone()),
            SqlValue::Uuid(self.principal),
        ];
        plan.push(Step::statement(sql, params, None));
        plan.touched::<PushSubscription>(Some(self.principal));
        Ok(())
    }
}

/// A route's handler answering the key a browser subscribes with, as `{"key": …}`.
pub async fn vapid_key(notify: Notify) -> Result<VapidKey> {
    let key = notify.vapid_key().ok_or_else(|| {
        Error::internal_message(
            "web push is not set up: set SOFIE_NOTIFY_VAPID_PUBLIC_KEY, _PRIVATE_KEY and _SUBJECT",
        )
    })?;
    Ok(VapidKey {
        key: key.to_owned(),
    })
}

#[derive(Serialize)]
pub struct VapidKey {
    key: String,
}

impl Outcome for VapidKey {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::data(self))
    }
}

impl sofie_db::ReadOnly for VapidKey {}

/// Committed with the handler's outcome; a statement, as the endpoint may be another principal's.
pub struct Subscribed {
    id: Uuid,
    principal: Uuid,
    registration: PushRegistration,
    now: Timestamp,
}

/// Aliased, so the upsert names the row it conflicts with the same under either database. A
/// browser another principal takes over gets a new id, so a push queued for the former one,
/// which names the old id, finds nothing.
fn subscribe_sql(dialect: &dyn Dialect) -> String {
    let at = |index| dialect.placeholder(index);
    format!(
        "INSERT INTO {table} AS existing \
         (id, version, principal, endpoint, p256dh, auth, created_at, gone_at) \
         VALUES ({id}, 1, {principal}, {endpoint}, {p256dh}, {auth}, {now}, NULL) \
         ON CONFLICT (endpoint) DO UPDATE SET \
         id = CASE WHEN existing.principal = EXCLUDED.principal THEN existing.id ELSE EXCLUDED.id END, \
         principal = EXCLUDED.principal, \
         p256dh = EXCLUDED.p256dh, auth = EXCLUDED.auth, created_at = EXCLUDED.created_at, \
         gone_at = NULL, version = existing.version + 1 \
         WHERE existing.auth = EXCLUDED.auth OR existing.gone_at IS NOT NULL",
        table = push_table(dialect),
        id = at(1),
        principal = at(2),
        endpoint = at(3),
        p256dh = at(4),
        auth = at(5),
        now = at(6),
    )
}

fn let_go_oldest_sql(dialect: &dyn Dialect) -> String {
    let at = |index| dialect.placeholder(index);
    let (principal, now, kept) = (at(1), at(2), at(3));
    let table = push_table(dialect);
    format!(
        "UPDATE {table} SET gone_at = {now}, version = version + 1 \
         WHERE principal = {principal} AND gone_at IS NULL AND id NOT IN (\
         SELECT id FROM {table} WHERE principal = {principal} AND gone_at IS NULL \
         ORDER BY created_at DESC, id DESC LIMIT {kept})"
    )
}

fn push_table(dialect: &dyn Dialect) -> String {
    dialect.qualified(PushSubscription::MODULE, PushSubscription::NAME)
}

impl Outcome for Subscribed {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        plan.push(Step::statement(
            subscribe_sql(plan.dialect()),
            vec![
                SqlValue::Uuid(self.id),
                SqlValue::Uuid(self.principal),
                SqlValue::Text(self.registration.endpoint.clone()),
                SqlValue::Text(self.registration.keys.p256dh.clone()),
                SqlValue::Text(self.registration.keys.auth.clone()),
                SqlValue::Timestamp(self.now),
            ],
            Some(1),
        ));
        plan.push(Step::statement(
            let_go_oldest_sql(plan.dialect()),
            vec![
                SqlValue::Uuid(self.principal),
                SqlValue::Timestamp(self.now),
                SqlValue::Int(MAXIMUM_BROWSERS),
            ],
            None,
        ));
        plan.touched::<PushSubscription>(Some(self.principal));
        Ok(())
    }
}

pub(crate) async fn subscriptions(read: &Read, principal: Uuid) -> Result<Vec<Uuid>> {
    let subscriptions = PushSubscription::query()
        .where_(PUSH_PRINCIPAL.eq(principal))
        .where_(PUSH_GONE_AT.is_null())
        .all(read)
        .await?;
    Ok(subscriptions
        .iter()
        .map(|subscription| subscription.meta.id())
        .collect())
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Pushing {
    pub(crate) subscription: Uuid,
    pub(crate) notice: Notice,
}

impl Resolve<JobContext> for Pushing {
    fn resolve(context: &mut JobContext) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(crate::job_input(context, JOB))
    }
}

/// A subscription the push service says is gone is kept as gone and never pushed to again.
pub(crate) async fn push(
    pushing: Pushing,
    read: Read,
    clock: Clock,
    setup: Dep<Setup>,
) -> Result<Option<PushSubscription>> {
    let Some(keys) = setup.vapid.as_deref() else {
        tracing::warn!("a push was due, but web push is not set up; set SOFIE_NOTIFY_VAPID_*");
        return Ok(None);
    };
    let subscription = PushSubscription::query()
        .where_(PushSubscription::ID.eq(Id::from_uuid(pushing.subscription)))
        .first(&read)
        .await?;
    let Some(mut subscription) = subscription.filter(|found| found.gone_at.is_none()) else {
        return Ok(None);
    };
    let now = clock.now().timestamp();
    let request = request(keys, &subscription, &pushing.notice, now)?;
    let sent = setup
        .http
        .untrusted()
        .post(&subscription.endpoint)
        .header("ttl", TIME_TO_LIVE)
        .header("content-encoding", "aes128gcm")
        .header("authorization", &request.authorization)
        .body("application/octet-stream", request.body)
        .send()
        .await;
    match answered(sent.map(|answer| answer.status()))? {
        Answered::Delivered => Ok(None),
        Answered::Gone => {
            subscription.gone_at = Some(now);
            Ok(Some(subscription))
        }
        Answered::Refused(status) => {
            refused(&subscription, status);
            Ok(None)
        }
    }
}

/// The service's origin only: the endpoint's path is the browser's capability to be pushed to.
fn refused(subscription: &PushSubscription, status: u16) {
    tracing::error!(
        status,
        push.subscription = %subscription.meta.id(),
        push.service = vapid::audience(&subscription.endpoint).unwrap_or_default(),
        "the push service refused a push; it is not retried"
    );
}

#[derive(Debug, PartialEq, Eq)]
enum Answered {
    Delivered,
    Gone,
    Refused(u16),
}

/// A guard refusal refuses every later push too, so it is gone as an expired one is; a 400, 401
/// or 403 is most often the sender's setup (a changed VAPID key) and would mark every one gone.
fn answered(sent: Result<u16>) -> Result<Answered> {
    match sent {
        Err(Error::BadRequest(_)) | Ok(404 | 410) => Ok(Answered::Gone),
        Err(error) => Err(error),
        Ok(200..=299) => Ok(Answered::Delivered),
        Ok(status @ (429 | 500..=599)) => Err(Error::Unavailable(format!(
            "the push service answered {status}"
        ))),
        Ok(status) => Ok(Answered::Refused(status)),
    }
}

struct PushRequest {
    authorization: String,
    body: Vec<u8>,
}

fn request(
    keys: &VapidKeys,
    subscription: &PushSubscription,
    notice: &Notice,
    now: Timestamp,
) -> Result<PushRequest> {
    let undecodable = || Error::internal_message("a stored push subscription's keys are malformed");
    let browser_key = decode(&URL_SAFE, &subscription.p256dh).ok_or_else(undecodable)?;
    let auth = decode(&URL_SAFE, &subscription.auth).ok_or_else(undecodable)?;
    Ok(PushRequest {
        authorization: keys.authorization(&subscription.endpoint, now)?,
        body: encrypt::encrypt(&browser_key, &auth, message(notice).as_bytes())?,
    })
}

/// The body is cut until the message fits one record; the title and link always fit.
fn message(notice: &Notice) -> String {
    let mut body: String = notice.body.chars().take(MAXIMUM_PUSHED_BODY).collect();
    loop {
        let message = json!({ "title": notice.title, "body": body, "link": notice.link });
        let message = message.to_string();
        let excess = message.len().saturating_sub(encrypt::MAXIMUM_PLAINTEXT);
        if excess == 0 || body.is_empty() {
            return message;
        }
        // JSON escapes a character to at most six bytes.
        for _ in 0..excess.div_ceil(6) {
            body.pop();
        }
    }
}

/// Shows each pushed notification and opens its link when clicked. Serve it from the
/// app's origin and register it with `navigator.serviceWorker.register`.
pub const SERVICE_WORKER: &str = r#"self.addEventListener("push", (event) => {
  const message = event.data ? event.data.json() : {};
  event.waitUntil(self.registration.showNotification(message.title || "", {
    body: message.body || "",
    data: { link: message.link || "/" },
  }));
});
self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  event.waitUntil(clients.openWindow(event.notification.data.link));
});
"#;

#[cfg(test)]
mod tests {
    use ring::agreement;
    use ring::rand::SystemRandom;
    use sofie_core::encoding::base64::encode;
    use sofie_db::Meta;

    use super::*;

    #[test]
    fn an_endpoint_refused_by_the_guard_or_answered_404_or_410_is_gone() {
        let guarded = Err(Error::BadRequest(
            "the outgoing request is refused".to_owned(),
        ));
        assert_eq!(answered(guarded).unwrap(), Answered::Gone);
        for status in [404, 410] {
            assert_eq!(answered(Ok(status)).unwrap(), Answered::Gone, "{status}");
        }
        assert_eq!(answered(Ok(201)).unwrap(), Answered::Delivered);
        for status in [400, 401, 403, 413] {
            assert_eq!(answered(Ok(status)).unwrap(), Answered::Refused(status));
        }
        for status in [429, 503] {
            assert!(matches!(answered(Ok(status)), Err(Error::Unavailable(_))));
        }
        let unreachable = Err(Error::Unavailable("connecting timed out".to_owned()));
        assert!(answered(unreachable).is_err());
    }

    #[test]
    fn a_refused_push_is_logged_with_its_subscription_and_push_service_only() {
        let subscription = subscription("https://push.example.test/send/secret-token");
        let telemetry = sofie_telemetry::capture();

        refused(&subscription, 403);

        let [event] = telemetry.events().try_into().ok().unwrap();
        let id = subscription.meta.id().to_string();
        assert_eq!(event.fields.get("push.subscription"), Some(&id));
        let service = event.fields.get("push.service").unwrap();
        assert_eq!(service, "https://push.example.test");
        assert!(!format!("{:?}", event.fields).contains("secret-token"));
    }

    fn subscription(endpoint: &str) -> PushSubscription {
        let random = SystemRandom::new();
        let browser =
            agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &random).unwrap();
        PushSubscription {
            meta: Meta::new().unwrap(),
            principal: Uuid::now_v7().unwrap(),
            endpoint: endpoint.to_owned(),
            p256dh: encode(&URL_SAFE, browser.compute_public_key().unwrap().as_ref()),
            auth: encode(&URL_SAFE, &[3; 16]),
            created_at: Timestamp::UNIX_EPOCH,
            gone_at: None,
        }
    }

    #[test]
    fn a_registration_holds_an_https_endpoint_and_well_formed_keys() {
        let random = SystemRandom::new();
        let browser =
            agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &random).unwrap();
        let p256dh = encode(&URL_SAFE, browser.compute_public_key().unwrap().as_ref());
        let registration = |endpoint: &str, auth: &str| {
            PushRegistration::from_json(json!({
                "endpoint": endpoint, "expirationTime": null,
                "keys": {"p256dh": p256dh, "auth": auth}
            }))
        };
        let auth = encode(&URL_SAFE, &[1; 16]);

        assert!(registration("https://push.example.test/abc", &auth).is_ok());
        assert!(registration("http://push.example.test/abc", &auth).is_err());
        assert!(registration("https://push.example.test/abc", "short").is_err());
    }

    #[test]
    fn a_push_carries_a_vapid_token_and_one_encrypted_record() {
        let (public, private) = vapid::tests::generated();
        let keys = VapidKeys::new(&public, &private, "mailto:ops@example.test").unwrap();
        let subscription = subscription("https://push.example.test/send/1");
        let notice = Notice::new("Shipped")
            .unwrap()
            .body(&"x".repeat(2000))
            .unwrap();

        let request = request(&keys, &subscription, &notice, Timestamp::UNIX_EPOCH).unwrap();

        assert!(request.authorization.starts_with("vapid t="));
        assert_eq!(&request.body[16..20], &4096u32.to_be_bytes());
        assert!(request.body.len() < 4096 + 86);
    }

    #[test]
    fn a_notice_whose_text_escapes_long_still_fits_one_record() {
        let notice = Notice::new(&"\"".repeat(200))
            .unwrap()
            .body(&"\u{1}".repeat(2000))
            .unwrap()
            .link(&format!("/{}", "\"".repeat(999)))
            .unwrap();

        let message = message(&notice);

        assert!(message.len() <= encrypt::MAXIMUM_PLAINTEXT);
        let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
        assert_eq!(parsed["title"], notice.title);
        assert_eq!(parsed["link"], notice.link.clone().unwrap());
        assert_ne!(parsed["body"], "");
    }
}
