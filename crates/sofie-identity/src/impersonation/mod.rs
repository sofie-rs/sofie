//! Signing in as a member, for a permitted principal of another area: each time recorded
//! with its reason and raised as an event, the session it opens marked as such to the member
//! and ended by itself after half an hour, and the impersonator signed back in at its end.

use std::time::Duration;

use sofie::core::Instant;
use sofie::db::{Outcome, Plan, Resolve, Stamp, Uuid};
use sofie::http::{
    Area, CurrentSession, NeedsPermission, Permission, Permissions, RequestContext, Routes, Shipped,
};
use sofie::prelude::*;

use crate::account::Account;
use crate::membership::{self, held};
use crate::signing_in::{SignedIn, sign_in};
use crate::{AccountArea, AccountId, MemberArea, Unscoped, account_of};

const READ: Permission = Permission::new("sofie_identity.impersonation.*.read");
const CREATE: Permission = Permission::new("sofie_identity.impersonation.*.create");
/// Long enough to look into a problem, short enough that a forgotten one ends soon.
const LASTS: Duration = Duration::from_mins(30);
const SHOWN: u64 = 50;
const NOT_A_MEMBER: &str = "has no account that is a member there";
const PERMITTED_ELSEWHERE: &str =
    "is a member of the area impersonating it, which nobody impersonates";
const NOT_IMPERSONATING: &str = "This session is no impersonation.";

/// The audit record: who signed in as whom, where, why, and until when. The member owns
/// it, so its own area reads and ends it.
#[derive(Aggregate)]
pub struct Impersonation {
    meta: Meta,
    #[owner]
    account: AccountId,
    impersonator: AccountId,
    /// As it was then, as an audit record keeps it.
    impersonator_email: Email,
    area: String,
    reason: String,
    session: Uuid,
    /// The impersonator's own session, which must still live for stopping to sign back in.
    started_from: Option<Uuid>,
    ends_at: Instant,
    ended_at: Option<Instant>,
    /// The member's permissions at the start, which the impersonator's covered; a request
    /// finding the member holding another ends the impersonation, as it may reach beyond them.
    granted: String,
}

const GRANTED_SEPARATOR: char = ' ';

impl Impersonation {
    pub(crate) fn end(&mut self, now: Instant) -> Result<()> {
        self.ended_at = Some(now);
        self.meta.raise(ImpersonationEnded {
            impersonation: self.id().uuid(),
        })
    }

    fn still_covers(&self, granted: &[String]) -> bool {
        granted.iter().all(|permission| {
            self.granted
                .split(GRANTED_SEPARATOR)
                .any(|held| held == permission)
        })
    }
}

/// Raised when an impersonation starts; an app keeps its own audit log by `#[on]`.
#[derive(Event)]
pub struct ImpersonationStarted {
    pub impersonation: Uuid,
    pub impersonator: Uuid,
    pub member: Uuid,
    pub area: String,
    pub reason: String,
}

#[derive(Event)]
pub struct ImpersonationEnded {
    pub impersonation: Uuid,
}

/// In the permitted area `S`: the record of impersonations, and starting one as a member of `M`.
pub fn routes<S, M>(routes: &mut Routes<S>)
where
    S: AccountArea,
    M: MemberArea,
{
    let describe: sofie::http::Describe =
        |screens, key| screens.describe_form::<Starting>(key, "impersonate");
    routes
        .shipped(
            "/impersonation",
            "impersonation",
            (include_str!("screen.html"), Some(describe)),
            record,
        )
        .name("list_impersonations")
        .permission(READ);
    routes
        .post("/impersonation", start::<S, M>)
        .name("impersonate")
        .permission(CREATE)
        .then_redirect(M::HOME);
}

/// In the member area `M`: ending the impersonation and returning to `S`.
pub fn stop_routes<M, S>(routes: &mut Routes<M>)
where
    M: MemberArea,
    S: Area,
{
    routes
        .post("/impersonation/stop", stop::<M, S>)
        .name("stop_impersonating")
        .then_redirect(&format!("{}/impersonation", S::PREFIX));
}

#[derive(ReadModel)]
pub struct Record {
    impersonations: Vec<Listed>,
}

#[derive(ReadModel)]
struct Listed {
    impersonator: Email,
    member: Email,
    area: String,
    reason: String,
    created_at: Option<Instant>,
    ended_at: Option<Instant>,
}

#[derive(ReadModel)]
#[read(from = Account)]
struct Member {
    id: AccountId,
    email: Email,
}

async fn record(read: Read, clock: Clock) -> Result<Record> {
    let now = clock.now();
    let held = Impersonation::query()
        .latest()
        .limit(SHOWN)
        .all(&read)
        .await?;
    let members = Member::query()
        .where_(Account::ID.in_(held.iter().map(|impersonation| impersonation.account)))
        .all(&read)
        .await?;
    let email_of = |account| {
        members
            .iter()
            .find(|member| member.id == account)
            .map(|member| member.email.clone())
            .ok_or(Error::NotFound)
    };
    let impersonations = held
        .into_iter()
        .map(|impersonation| {
            Ok(Listed {
                member: email_of(impersonation.account)?,
                created_at: impersonation.created_at(),
                impersonator: impersonation.impersonator_email,
                area: impersonation.area,
                reason: impersonation.reason,
                ended_at: impersonation
                    .ended_at
                    .or(Some(impersonation.ends_at).filter(|ends_at| *ends_at <= now)),
            })
        })
        .collect::<Result<_>>()?;
    Ok(Record { impersonations })
}

#[derive(Input)]
pub struct Starting {
    email: Email,
    /// Kept in the record, so every impersonation says what it was for.
    #[min(3)]
    #[max(500)]
    reason: String,
}

pub struct Started {
    impersonation: Impersonation,
    signed_in: SignedIn,
}

async fn start<S, M>(
    principal: Principal<S>,
    permissions: Permissions,
    starting: Starting,
    read: Read,
    session: Session,
    clock: Clock,
) -> Result<Started>
where
    S: AccountArea,
    M: MemberArea,
{
    let reason = starting.reason.trim();
    if reason.chars().count() < 3 {
        return Err(Error::invalid(
            "reason",
            "must say what the impersonation is for",
        ));
    }
    let impersonator = Account::stored(&read, account_of(&principal)).await?;
    let member = Account::of_email(&read, starting.email)
        .await?
        .filter(|member| !member.is_closed())
        .ok_or_else(|| Error::invalid("email", NOT_A_MEMBER))?;
    if held(&read, member.id(), M::NAME).await?.is_none() {
        return Err(Error::invalid("email", NOT_A_MEMBER));
    }
    if held(&read, member.id(), S::NAME).await?.is_some() {
        return Err(Error::invalid("email", PERMITTED_ELSEWHERE));
    }
    let granted = refuse_unless_within(&read, &permissions, member.id(), M::NAME).await?;
    let now = clock.now();
    let ends_at = now.checked_add(LASTS)?;
    let started_from = session.current();
    let signed_in =
        sign_in::<M>(&session, member.id(), &clock)?.impersonated(impersonator.email(), ends_at);
    let current = signed_in.session();
    let mut impersonation = Impersonation {
        meta: Meta::new()?,
        account: member.id(),
        impersonator: impersonator.id(),
        impersonator_email: impersonator.email().clone(),
        area: M::NAME.to_owned(),
        reason: reason.to_owned(),
        session: current,
        started_from,
        ends_at,
        ended_at: None,
        granted: granted.join(&GRANTED_SEPARATOR.to_string()),
    };
    impersonation.meta.raise(ImpersonationStarted {
        impersonation: impersonation.id().uuid(),
        impersonator: impersonator.id().uuid(),
        member: member.id().uuid(),
        area: M::NAME.to_owned(),
        reason: impersonation.reason.clone(),
    })?;
    Ok(Started {
        impersonation,
        signed_in,
    })
}

/// The rule role grants follow: nobody acts beyond its own permissions, here as another.
async fn refuse_unless_within(
    read: &Read,
    impersonator: &Permissions,
    member: AccountId,
    area: &str,
) -> Result<Vec<String>> {
    let granted = membership::roles::granted(read, area, member).await?;
    match granted
        .iter()
        .find(|permission| !impersonator.can_named(permission))
    {
        Some(beyond) => Err(Error::invalid(
            "email",
            format!(
                "holds `{beyond}`, which your own permissions do not cover, so you cannot impersonate it"
            ),
        )),
        None => Ok(granted),
    }
}

impl Outcome for Started {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.impersonation.plan(plan)?;
        self.signed_in.plan(plan)
    }

    fn committed(&mut self, stamp: Stamp) {
        self.impersonation.committed(stamp);
        self.signed_in.committed(stamp);
    }
}

async fn stop<M, S>(
    _member: Principal<M>,
    read: Read,
    unscoped: Unscoped,
    session: Session,
    clock: Clock,
) -> Result<()>
where
    M: MemberArea,
    S: Area,
{
    let mut impersonation = of_session(&read, session.current())
        .await?
        .ok_or_else(|| Error::Rule(NOT_IMPERSONATING.to_owned()))?;
    impersonation.end(clock.now())?;
    let sessions = crate::sessions::ending_current(&read, &session, &clock).await?;
    let signed_in = if returns::<S>(&unscoped.0.read(), &impersonation).await? {
        Some(sign_in::<S>(&session, impersonation.impersonator, &clock)?)
    } else {
        session.sign_out();
        None
    };
    unscoped
        .0
        .commit(&mut (impersonation, sessions, signed_in))
        .await
}

/// The member's session outlives a password reset or "sign out everywhere" of the
/// impersonator's account, so only the impersonator's own session still live signs it back in.
async fn returns<S: Area>(read: &Read, impersonation: &Impersonation) -> Result<bool> {
    let Some(started_from) = impersonation.started_from else {
        return Ok(false);
    };
    let impersonator = impersonation.impersonator;
    if crate::sessions::account_of(read, started_from).await? != Some(impersonator) {
        return Ok(false);
    }
    Ok(held(read, impersonator, S::NAME).await?.is_some())
}

#[cfg(feature = "account-data")]
pub(crate) async fn exported(read: &Read, account: AccountId) -> Result<sofie::serde_json::Value> {
    let impersonations = Impersonation::query()
        .where_(Impersonation::ACCOUNT.eq(account))
        .all(read)
        .await?;
    Ok(sofie::serde_json::json!(
        impersonations
            .iter()
            .map(|impersonation| sofie::serde_json::json!({
                "by": impersonation.impersonator_email.as_str(),
                "area": impersonation.area,
                "reason": impersonation.reason,
                "ends_at": impersonation.ends_at,
                "ended_at": impersonation.ended_at,
            }))
            .collect::<Vec<_>>()
    ))
}

/// Each impersonation in one of `sessions`, ended on the record, for whatever ended them.
pub(crate) async fn ending_in_sessions(
    read: &Read,
    sessions: &[Uuid],
    now: Instant,
) -> Result<Vec<Impersonation>> {
    if sessions.is_empty() {
        return Ok(Vec::new());
    }
    let mut open = Impersonation::query()
        .where_(Impersonation::SESSION.in_(sessions.to_vec()))
        .where_(Impersonation::ENDED_AT.is_null())
        .all(read)
        .await?;
    for impersonation in &mut open {
        impersonation.end(now)?;
    }
    Ok(open)
}

pub(crate) async fn sessions_started_by(read: &Read, impersonator: AccountId) -> Result<Vec<Uuid>> {
    let open = Impersonation::query()
        .where_(Impersonation::IMPERSONATOR.eq(impersonator))
        .where_(Impersonation::ENDED_AT.is_null())
        .all(read)
        .await?;
    Ok(open
        .into_iter()
        .map(|impersonation| impersonation.session)
        .collect())
}

async fn of_session(read: &Read, session: Option<Uuid>) -> Result<Option<Impersonation>> {
    let Some(session) = session else {
        return Ok(None);
    };
    let found = Impersonation::query()
        .where_(Impersonation::SESSION.eq(session))
        .first(read)
        .await?;
    Ok(found.filter(|impersonation| impersonation.ended_at.is_none()))
}

pub(crate) enum Checked {
    Absent,
    By(Uuid),
    Beyond(Box<Impersonation>),
}

/// Read on each request of an impersonated session only, so no other pays for it.
pub(crate) async fn checked(read: &Read, session: Uuid) -> Result<Checked> {
    let Some(impersonation) = of_session(read, Some(session)).await? else {
        return Ok(Checked::Absent);
    };
    let granted =
        membership::roles::granted(read, &impersonation.area, impersonation.account).await?;
    if impersonation.still_covers(&granted) {
        return Ok(Checked::By(impersonation.impersonator.uuid()));
    }
    Ok(Checked::Beyond(Box::new(impersonation)))
}

/// For a member area's screens: who is impersonating its member, if anyone, so a page can
/// show it and offer `/impersonation/stop`.
pub struct Impersonating(Option<String>);

impl Impersonating {
    #[must_use]
    pub fn by(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

impl<A: Area, M> Resolve<RequestContext<A, M>> for Impersonating {
    async fn resolve(context: &mut RequestContext<A, M>) -> Result<Self> {
        let current = CurrentSession::resolve(context).await?;
        let read = Read::resolve(context).await?;
        let impersonation = of_session(&read, current.id()).await?;
        Ok(Self(impersonation.map(|impersonation| {
            impersonation.impersonator_email.as_str().to_owned()
        })))
    }
}
