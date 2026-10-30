use sofie::db::{Outcome, Plan, Reply, Resolve, Stamp};
use sofie::http::{PrincipalRecord, RequestContext, SignInRight};
use sofie::prelude::*;
use sofie::serde_json::json;

use crate::{AccountId, MemberArea};

struct Identity;

impl sofie::http::Admitter for Identity {}

const RIGHT: SignInRight = SignInRight::of::<Identity>();
#[cfg(any(feature = "passkeys", feature = "oauth", feature = "totp"))]
const SIGN_IN_AGAIN: &str = "Sign in again first: this needs a sign-in from the last ten minutes.";
#[cfg(feature = "email-verification")]
const UNCONFIRMED: &str = "Confirm your email address first: follow the link mailed to it.";

pub struct SignedIn {
    #[cfg(feature = "impersonation")]
    session: sofie::db::Uuid,
    #[cfg(feature = "sessions")]
    record: crate::sessions::AccountSession,
}

/// Every way of signing in ends here, after the account proved itself and its membership.
pub(crate) fn sign_in<A: Area>(
    session: &Session,
    account: AccountId,
    clock: &Clock,
) -> Result<SignedIn> {
    sign_in_for::<A>(
        session,
        RIGHT,
        Id::from_uuid(account.uuid()),
        account,
        clock,
    )
}

/// Signs `account` in to an area whose principal is another of its records, such as an
/// organization it belongs to; the session is listed as the account's. The caller, an
/// `Admitter` of its own, has checked that the account may act as `principal`.
pub fn sign_in_for<A: Area>(
    session: &Session,
    right: SignInRight,
    principal: Id<PrincipalRecord<A>>,
    account: AccountId,
    clock: &Clock,
) -> Result<SignedIn> {
    let id = session.sign_in::<A>(right, principal)?;
    #[cfg(not(feature = "sessions"))]
    let _ = (id, account, clock);
    Ok(SignedIn {
        #[cfg(feature = "impersonation")]
        session: id,
        #[cfg(feature = "sessions")]
        record: crate::sessions::started(
            id,
            account,
            A::NAME,
            principal.uuid(),
            session.user_agent().as_deref(),
            clock.now(),
        )?,
    })
}

/// Signs the impersonator's own session back in, so stopping proves no recent sign-in.
#[cfg(feature = "impersonation")]
pub(crate) fn resume<A: Area>(session: &Session, resumed: &crate::sessions::Resumable) {
    session.resume::<A>(
        RIGHT,
        resumed.session,
        Id::from_uuid(resumed.principal),
        resumed.signed_in_at.timestamp(),
    );
}

#[cfg(feature = "impersonation")]
impl SignedIn {
    pub(crate) fn session(&self) -> sofie::db::Uuid {
        self.session
    }

    pub(crate) fn impersonated(mut self, by: &Email, until: sofie::core::Instant) -> Self {
        self.record.impersonated(by, until);
        self
    }
}

impl Outcome for SignedIn {
    #[cfg(feature = "sessions")]
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.record.plan(plan)
    }

    #[cfg(feature = "sessions")]
    fn committed(&mut self, stamp: Stamp) {
        self.record.committed(stamp);
    }
}

/// Refused in an impersonation, so nothing it changes outlasts its half hour and its record.
#[cfg(any(
    feature = "passkeys",
    feature = "oauth",
    feature = "totp",
    feature = "account-data",
    feature = "session-screens",
    feature = "token-screens"
))]
pub(crate) struct OwnSession;

#[cfg(any(
    feature = "passkeys",
    feature = "oauth",
    feature = "totp",
    feature = "account-data",
    feature = "session-screens",
    feature = "token-screens"
))]
impl<A: Area, M> Resolve<RequestContext<A, M>> for OwnSession {
    #[cfg_attr(
        not(feature = "impersonation"),
        expect(clippy::unused_async_trait_impl)
    )]
    async fn resolve(context: &mut RequestContext<A, M>) -> Result<Self> {
        #[cfg(feature = "impersonation")]
        if crate::impersonation::Impersonating::resolve(context)
            .await?
            .by()
            .is_some()
        {
            return Err(Error::Rule(
                "An impersonation cannot change how this account signs in, end its sessions, export or close it."
                    .to_owned(),
            ));
        }
        #[cfg(not(feature = "impersonation"))]
        let _ = context;
        Ok(Self)
    }
}

/// So a session taken over adds no way to sign in that outlasts it, nor a second factor.
#[cfg(any(feature = "passkeys", feature = "oauth", feature = "totp"))]
pub(crate) struct RecentSignIn;

#[cfg(any(feature = "passkeys", feature = "oauth", feature = "totp"))]
impl<A: Area> Resolve<RequestContext<A>> for RecentSignIn {
    async fn resolve(context: &mut RequestContext<A>) -> Result<Self> {
        if !SignedInRecently::resolve(context).await?.0 {
            return Err(Error::Rule(SIGN_IN_AGAIN.to_owned()));
        }
        Ok(Self)
    }
}

/// The own session, and whether it signed in within the last ten minutes.
#[cfg(any(feature = "passkeys", feature = "oauth", feature = "totp"))]
pub(crate) struct SignedInRecently(pub(crate) bool);

#[cfg(any(feature = "passkeys", feature = "oauth", feature = "totp"))]
impl<A: Area, M> Resolve<RequestContext<A, M>> for SignedInRecently {
    async fn resolve(context: &mut RequestContext<A, M>) -> Result<Self> {
        OwnSession::resolve(context).await?;
        let current = sofie::http::CurrentSession::resolve(context).await?.id();
        let now = Clock::resolve(context).await?.now();
        let read = Read::resolve(context).await?;
        let Some(current) = current else {
            return Ok(Self(false));
        };
        crate::sessions::signed_in_recently(&read, current, now)
            .await
            .map(Self)
    }
}

pub(crate) struct Door {
    session: Session,
    read: Read,
    clock: Clock,
    #[cfg(feature = "totp")]
    urls: sofie::http::SignedUrls,
    #[cfg(feature = "totp")]
    next: sofie::http::Next,
}

impl Resolve<RequestContext<Web>> for Door {
    async fn resolve(context: &mut RequestContext<Web>) -> Result<Self> {
        Ok(Self {
            session: Session::resolve(context).await?,
            read: Read::resolve(context).await?,
            clock: Clock::resolve(context).await?,
            #[cfg(feature = "totp")]
            urls: sofie::http::SignedUrls::resolve(context).await?,
            #[cfg(feature = "totp")]
            next: sofie::http::Next::resolve(context).await?,
        })
    }
}

/// Replies where the browser goes next only when that is not where signing in goes:
/// a route with `then_redirect_or_follow` keeps its `next`. `saved` is committed with it.
pub(crate) struct Entered<T = ()> {
    signed_in: Option<SignedIn>,
    #[cfg(feature = "totp")]
    pending: Option<crate::account_link::AccountLink>,
    location: Option<String>,
    saved: T,
}

impl Door {
    pub(crate) fn read(&self) -> &Read {
        &self.read
    }

    #[cfg(any(feature = "links", feature = "passkeys", feature = "oauth"))]
    pub(crate) fn clock(&self) -> &Clock {
        &self.clock
    }

    #[cfg(any(feature = "passkeys", feature = "oauth"))]
    pub(crate) fn session(&self) -> &Session {
        &self.session
    }

    pub(crate) async fn enter<A: MemberArea>(&self, account: AccountId) -> Result<Entered> {
        self.enter_after::<A>(account, false).await
    }

    /// `second_proved` when the first step was a second factor too, as a verified passkey is.
    /// An area needing a confirmed address admits no account without one, however it signed in.
    pub(crate) async fn enter_after<A: MemberArea>(
        &self,
        account: AccountId,
        second_proved: bool,
    ) -> Result<Entered> {
        #[cfg(feature = "email-verification")]
        if A::VERIFIED_EMAIL
            && crate::account::Account::stored(&self.read, account)
                .await?
                .verified_at()
                .is_none()
        {
            return Err(Error::Rule(UNCONFIRMED.to_owned()));
        }
        self.admit::<A>(account, second_proved).await
    }

    /// For a mailed link or a provider's confirmed address, which confirm the account's
    /// address in the same commit; `distrusted` when that commit removes its second factor.
    #[cfg(any(feature = "links", feature = "oauth"))]
    pub(crate) async fn enter_confirming<A: MemberArea>(
        &self,
        account: AccountId,
        distrusted: bool,
    ) -> Result<Entered> {
        self.admit::<A>(account, distrusted).await
    }

    #[cfg_attr(not(feature = "totp"), expect(clippy::unused_async_trait_impl))]
    async fn admit<A: MemberArea>(
        &self,
        account: AccountId,
        second_proved: bool,
    ) -> Result<Entered> {
        #[cfg(not(feature = "totp"))]
        let _ = second_proved;
        #[cfg(feature = "totp")]
        if !second_proved
            && crate::two_factor::required(&self.read, account)
                .await?
                .is_some()
        {
            let (pending, location) =
                crate::two_factor::asking::<A>(account, &self.urls, &self.clock, &self.next)?;
            return Ok(Entered {
                signed_in: None,
                pending: Some(pending),
                location: Some(location),
                saved: (),
            });
        }
        Ok(Entered {
            signed_in: Some(sign_in::<A>(&self.session, account, &self.clock)?),
            #[cfg(feature = "totp")]
            pending: None,
            location: None,
            saved: (),
        })
    }
}

impl Entered {
    /// For a caller that has no `next` to keep, such as `engine.js`'s passkey ceremony.
    #[cfg(feature = "passkeys")]
    pub(crate) fn located<A: MemberArea>(self) -> Self {
        Self {
            location: self.location.or_else(|| Some(A::HOME.to_owned())),
            ..self
        }
    }

    pub(crate) fn saving<T: Outcome>(self, saved: T) -> Entered<T> {
        Entered {
            signed_in: self.signed_in,
            #[cfg(feature = "totp")]
            pending: self.pending,
            location: self.location,
            saved,
        }
    }
}

impl<T: Outcome> Outcome for Entered<T> {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.saved.plan(plan)?;
        if let Some(signed_in) = &self.signed_in {
            signed_in.plan(plan)?;
        }
        #[cfg(feature = "totp")]
        if let Some(pending) = &self.pending {
            pending.plan(plan)?;
        }
        Ok(())
    }

    fn committed(&mut self, stamp: Stamp) {
        self.saved.committed(stamp);
        if let Some(signed_in) = &mut self.signed_in {
            signed_in.committed(stamp);
        }
        #[cfg(feature = "totp")]
        if let Some(pending) = &mut self.pending {
            pending.committed(stamp);
        }
    }

    fn reply(self) -> Result<Reply> {
        match self.location {
            Some(location) => Ok(Reply::Json(json!({
                "location": location,
                "second_factor": self.signed_in.is_none(),
            }))),
            None => Ok(Reply::Empty),
        }
    }
}
