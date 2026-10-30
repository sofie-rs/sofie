#[cfg(feature = "session-screens")]
mod screen;

use std::time::Duration;

use sofie::core::Instant;
use sofie::db::{Db, Query, Uuid};
use sofie::http::SessionRecord;
use sofie::prelude::*;

#[cfg(feature = "session-screens")]
pub use screen::routes;

use crate::AccountId;

/// Kept short enough to show in a list, without anything that would mislead there.
const DEVICE_CHARACTERS: usize = 200;
/// A write per request would cost more than the request itself.
const LAST_SEEN_PRECISION: Duration = Duration::from_secs(60);
/// A cookie is sealed as the response is written, after its record: kept a while past the
/// lifetime, so no ended record goes while a cookie it refuses still opens.
const PRUNE_MARGIN: Duration = Duration::from_hours(1);
#[cfg(any(feature = "passkeys", feature = "oauth", feature = "totp"))]
const RECENT_SIGN_IN: Duration = Duration::from_mins(10);

/// Ended, not deleted, so a cookie still carrying it is refused until it expires by itself.
#[derive(Aggregate)]
pub struct AccountSession {
    meta: Meta,
    #[owner]
    account: AccountId,
    session: Uuid,
    area: String,
    /// What it signed in as, such as an organization the account acts for; none before it was kept.
    principal: Option<Uuid>,
    device: Option<String>,
    last_seen_at: Instant,
    ended_at: Option<Instant>,
    /// Ends by itself then, as an impersonation's does.
    expires_at: Option<Instant>,
}

impl AccountSession {
    #[must_use]
    pub fn area(&self) -> &str {
        &self.area
    }

    #[must_use]
    pub fn device(&self) -> Option<&str> {
        self.device.as_deref()
    }

    #[must_use]
    pub fn last_seen_at(&self) -> Instant {
        self.last_seen_at
    }

    fn end(&mut self, now: Instant) {
        self.ended_at.get_or_insert(now);
    }

    #[cfg(feature = "impersonation")]
    pub(crate) fn resumable(&self, account: AccountId, area: &str) -> Option<Resumable> {
        if self.account != account || self.area != area || self.ended_at.is_some() {
            return None;
        }
        Some(Resumable {
            session: self.session,
            principal: self.principal.unwrap_or_else(|| account.uuid()),
            signed_in_at: self.created_at()?,
        })
    }

    #[cfg(feature = "impersonation")]
    pub(crate) fn impersonated(&mut self, by: &Email, until: Instant) {
        self.device = Some(format!("Impersonated by {}", by.as_str()));
        self.expires_at = Some(until);
    }
}

#[cfg(feature = "impersonation")]
pub(crate) struct Resumable {
    pub(crate) session: Uuid,
    pub(crate) principal: Uuid,
    pub(crate) signed_in_at: Instant,
}

pub(crate) fn started(
    session: Uuid,
    account: AccountId,
    area: &str,
    principal: Uuid,
    user_agent: Option<&str>,
    now: Instant,
) -> Result<AccountSession> {
    Ok(AccountSession {
        meta: Meta::new()?,
        account,
        session,
        area: area.to_owned(),
        principal: Some(principal),
        device: user_agent.map(device),
        last_seen_at: now,
        ended_at: None,
        expires_at: None,
    })
}

fn device(user_agent: &str) -> String {
    user_agent
        .chars()
        .filter(|character| !sofie::core::misleads_display(*character))
        .take(DEVICE_CHARACTERS)
        .collect()
}

/// A session the store never recorded is live: only an ended one is refused.
pub(crate) async fn record(
    db: &Db,
    session: Uuid,
    seen: Option<sofie::core::Timestamp>,
) -> Result<SessionRecord> {
    let read = db.read();
    let Some(mut record) = recorded(&read, session).await? else {
        return Ok(SessionRecord::Unrecorded);
    };
    if record.ended_at.is_some() {
        return Ok(SessionRecord::Ended);
    }
    // Only an impersonation's session ends by itself, so no other is looked up.
    #[cfg(feature = "impersonation")]
    let impersonator = match record.expires_at {
        Some(_) => match crate::impersonation::checked(&read, session).await? {
            crate::impersonation::Checked::Absent => None,
            crate::impersonation::Checked::By(impersonator) => Some(impersonator),
            crate::impersonation::Checked::Beyond(impersonation) => {
                return ending_impersonated(db, record, *impersonation).await;
            }
        },
        None => None,
    };
    #[cfg(not(feature = "impersonation"))]
    let impersonator = None;
    let held = SessionRecord::Live {
        account: record.account.uuid(),
        expires_at: record.expires_at,
        impersonator,
    };
    let Some(seen) = seen.map(Instant::from) else {
        return Ok(held);
    };
    if !held.is_live(seen) || seen < record.last_seen_at.checked_add(LAST_SEEN_PRECISION)? {
        return Ok(held);
    }
    record.last_seen_at = seen;
    match db.commit(&mut record).await {
        // Ended, or seen by a concurrent request, in between: the next request tells which.
        Err(Error::Conflict) | Ok(()) => Ok(held),
        Err(error) => Err(error),
    }
}

#[cfg(feature = "impersonation")]
async fn ending_impersonated(
    db: &Db,
    mut record: AccountSession,
    mut impersonation: crate::impersonation::Impersonation,
) -> Result<SessionRecord> {
    let now = db.clock().now();
    impersonation.end(now)?;
    record.end(now);
    match db.commit(&mut (record, impersonation)).await {
        // Ended by a concurrent request already.
        Err(Error::Conflict) | Ok(()) => Ok(SessionRecord::Ended),
        Err(error) => Err(error),
    }
}

/// A record unseen for longer than a cookie lives can refuse no cookie, ended or not.
pub(crate) async fn prune(prune: &sofie::db::Prune, clock: &Clock) -> Result<()> {
    let kept = sofie::auth::SessionClaims::ABSOLUTE_LIFETIME + PRUNE_MARGIN;
    let before = clock.now().checked_sub(kept)?;
    prune
        .before(AccountSession::LAST_SEEN_AT, before)
        .await
        .map(drop)
}

/// The account a live session signed in, read beyond the request's reach.
pub async fn account_of(read: &Read, session: Uuid) -> Result<Option<AccountId>> {
    let record = recorded(read, session).await?;
    Ok(record
        .filter(|record| record.ended_at.is_none())
        .map(|record| record.account))
}

pub(crate) async fn recorded(read: &Read, session: Uuid) -> Result<Option<AccountSession>> {
    AccountSession::query()
        .where_(AccountSession::SESSION.eq(session))
        .first(read)
        .await
}

#[cfg(any(feature = "passkeys", feature = "oauth", feature = "totp"))]
pub(crate) async fn signed_in_recently(read: &Read, session: Uuid, now: Instant) -> Result<bool> {
    AccountSession::query()
        .where_(AccountSession::SESSION.eq(session))
        .where_(AccountSession::ENDED_AT.is_null())
        .where_(AccountSession::CREATED_AT.gt(now.checked_sub(RECENT_SIGN_IN)?))
        .exists(read)
        .await
}

pub(crate) async fn ending_current(
    read: &Read,
    session: &Session,
    clock: &Clock,
) -> Result<Vec<AccountSession>> {
    let Some(current) = session.current() else {
        return Ok(Vec::new());
    };
    let mut ended: Vec<AccountSession> = recorded(read, current).await?.into_iter().collect();
    for record in &mut ended {
        record.end(clock.now());
    }
    Ok(ended)
}

/// The impersonations in sessions just ended, ended with them, so the record says when.
#[derive(Default)]
pub(crate) struct Impersonations {
    #[cfg(feature = "impersonation")]
    ended: Vec<crate::impersonation::Impersonation>,
}

impl sofie::db::Outcome for Impersonations {
    fn plan(&self, plan: &mut sofie::db::Plan<'_>) -> Result<()> {
        #[cfg(feature = "impersonation")]
        self.ended.plan(plan)?;
        #[cfg(not(feature = "impersonation"))]
        let _ = plan;
        Ok(())
    }

    fn committed(&mut self, stamp: sofie::db::Stamp) {
        #[cfg(feature = "impersonation")]
        self.ended.committed(stamp);
        #[cfg(not(feature = "impersonation"))]
        let _ = stamp;
    }
}

pub(crate) async fn impersonations_in(
    read: &Read,
    ended: &[AccountSession],
    now: Instant,
) -> Result<Impersonations> {
    #[cfg(feature = "impersonation")]
    {
        let sessions: Vec<Uuid> = ended.iter().map(|record| record.session).collect();
        let ended = crate::impersonation::ending_in_sessions(read, &sessions, now).await?;
        Ok(Impersonations { ended })
    }
    #[cfg(not(feature = "impersonation"))]
    {
        let _ = (read, ended, now);
        Ok(Impersonations::default())
    }
}

/// The account's live sessions but `keep`, and those of the impersonations it started.
pub(crate) async fn ending_all(
    read: &Read,
    account: AccountId,
    keep: Option<Uuid>,
    now: Instant,
) -> Result<Ended> {
    #[cfg_attr(not(feature = "impersonation"), expect(unused_mut))]
    let mut owned = AccountSession::ACCOUNT.eq(account);
    #[cfg(feature = "impersonation")]
    {
        let started = crate::impersonation::sessions_started_by(read, account).await?;
        owned = owned.or(AccountSession::SESSION.in_(started));
    }
    let mut which = AccountSession::query().where_(owned);
    if let Some(keep) = keep {
        which = which.where_(AccountSession::SESSION.ne(keep));
    }
    ending(read, which, now).await
}

/// Each of the account's live sessions in `area`, and those of the impersonations it started,
/// ended: for a membership of it removed, committed with the removal.
pub async fn ending_in(read: &Read, account: AccountId, area: &str, now: Instant) -> Result<Ended> {
    let which = AccountSession::query()
        .where_(AccountSession::ACCOUNT.eq(account))
        .where_(AccountSession::AREA.eq(area.to_owned()));
    #[cfg_attr(not(feature = "impersonation"), expect(unused_mut))]
    let mut ended = ending(read, which, now).await?;
    #[cfg(feature = "impersonation")]
    {
        let started = crate::impersonation::sessions_started_by(read, account).await?;
        let impersonated = AccountSession::query().where_(AccountSession::SESSION.in_(started));
        ended.add(ending(read, impersonated, now).await?);
    }
    Ok(ended)
}

/// The accounts' live sessions in `area` signed in as `principal`, ended: for accounts no
/// longer acting for that principal, such as the members of an organization deleted. One
/// recorded without its principal ends too, as it may be that one.
pub async fn ending_as(
    read: &Read,
    accounts: &[AccountId],
    area: &str,
    principal: Uuid,
    now: Instant,
) -> Result<Ended> {
    let signed_in = AccountSession::PRINCIPAL.is_null();
    let which = AccountSession::query()
        .where_(AccountSession::ACCOUNT.in_(accounts.iter().copied()))
        .where_(AccountSession::AREA.eq(area.to_owned()))
        .where_(signed_in.or(AccountSession::PRINCIPAL.eq(Some(principal))));
    ending(read, which, now).await
}

pub struct Ended {
    sessions: Vec<AccountSession>,
    impersonations: Impersonations,
}

#[cfg(feature = "impersonation")]
impl Ended {
    fn add(&mut self, other: Self) {
        self.sessions.extend(other.sessions);
        self.impersonations.ended.extend(other.impersonations.ended);
    }
}

impl sofie::db::Outcome for Ended {
    fn plan(&self, plan: &mut sofie::db::Plan<'_>) -> Result<()> {
        self.sessions.plan(plan)?;
        self.impersonations.plan(plan)
    }

    fn committed(&mut self, stamp: sofie::db::Stamp) {
        self.sessions.committed(stamp);
        self.impersonations.committed(stamp);
    }
}

async fn ending(read: &Read, which: Query<AccountSession>, now: Instant) -> Result<Ended> {
    let mut sessions = which
        .where_(AccountSession::ENDED_AT.is_null())
        .all(read)
        .await?;
    for record in &mut sessions {
        record.end(now);
    }
    let impersonations = impersonations_in(read, &sessions, now).await?;
    Ok(Ended {
        sessions,
        impersonations,
    })
}

#[cfg(feature = "account-data")]
pub(crate) async fn exported(read: &Read, account: AccountId) -> Result<sofie::serde_json::Value> {
    let sessions = AccountSession::query()
        .where_(AccountSession::ACCOUNT.eq(account))
        .all(read)
        .await?;
    Ok(sofie::serde_json::json!(
        sessions
            .iter()
            .map(|session| sofie::serde_json::json!({
                "area": session.area,
                "device": session.device,
                "last_seen_at": session.last_seen_at,
                "ended_at": session.ended_at,
            }))
            .collect::<Vec<_>>()
    ))
}

/// Every session ends as signing out everywhere ends them, and no record keeps the device it
/// was on.
#[cfg(feature = "account-data")]
pub(crate) async fn closing(read: &Read, account: AccountId, now: Instant) -> Result<Ended> {
    let mut ended = ending_all(read, account, None, now).await?;
    let earlier = AccountSession::query()
        .where_(AccountSession::ACCOUNT.eq(account))
        .where_(AccountSession::ENDED_AT.is_not_null())
        .where_(AccountSession::DEVICE.is_not_null())
        .all(read)
        .await?;
    ended.sessions.extend(earlier);
    for session in &mut ended.sessions {
        session.device = None;
    }
    Ok(ended)
}
