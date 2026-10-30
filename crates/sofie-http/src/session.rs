use std::sync::{Arc, Mutex};

use http::header::SET_COOKIE;
use http::{HeaderMap, HeaderValue};
use sofie_auth::{OpenError, SessionClaims, SigningKeys};
use sofie_core::{Result, Timestamp, Uuid, unpoisoned};
#[cfg(any(
    feature = "permissions",
    feature = "tokens",
    feature = "shield",
    feature = "session-list"
))]
use sofie_db::BoxFuture;
use sofie_db::{Id, Resolve};

use crate::answer::{self, Response};
use crate::caller::Caller;
use crate::cookie::CookieNames;
use crate::{Area, PrincipalRecord, RequestContext};

pub trait AreaMembers: Send + Sync + 'static {
    #[cfg(feature = "permissions")]
    fn grants<'a>(
        &'a self,
        read: &'a sofie_db::Read,
        area: &'static str,
        principal: Uuid,
    ) -> BoxFuture<'a, Result<Vec<String>>> {
        let _ = (read, area, principal);
        Box::pin(std::future::ready(Ok(Vec::new())))
    }

    #[cfg(feature = "tokens")]
    fn bearer<'a>(
        &'a self,
        db: &'a sofie_db::Db,
        area: &'static str,
        digest: &'a str,
    ) -> BoxFuture<'a, Result<Option<crate::Bearer>>> {
        let _ = (db, area, digest);
        Box::pin(std::future::ready(Ok(None)))
    }

    #[cfg(feature = "shield")]
    fn sign_in_locked<'a>(
        &'a self,
        db: &'a sofie_db::Db,
        account: Uuid,
        until: Timestamp,
    ) -> BoxFuture<'a, Result<()>> {
        let _ = (db, account, until);
        Box::pin(std::future::ready(Ok(())))
    }

    /// Asked for a session a request's cookie names when the cache does not hold it, and
    /// whenever the cookie is refreshed (`seen` set, at most every five minutes). A session no
    /// longer live is signed out.
    #[cfg(feature = "session-list")]
    fn session_record<'a>(
        &'a self,
        db: &'a sofie_db::Db,
        session: Uuid,
        seen: Option<Timestamp>,
    ) -> BoxFuture<'a, Result<SessionRecord>> {
        let _ = (db, session, seen);
        Box::pin(std::future::ready(Ok(SessionRecord::Unrecorded)))
    }

    /// The table of `session_record`'s records, written each time one ends, owned by its
    /// account. With it, records are cached until such a write, here or on another instance.
    #[cfg(feature = "session-list")]
    fn session_topic(&self) -> Option<sofie_db::Topic> {
        None
    }
}

/// What a session store holds of a session.
#[cfg(feature = "session-list")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionRecord {
    /// Signed in before the store recorded sessions: live until its cookie expires.
    Unrecorded,
    Live {
        account: Uuid,
        /// Ends by itself then, as an impersonation's does.
        expires_at: Option<sofie_core::Instant>,
        /// The account signed in as `account`; each request it makes is logged.
        impersonator: Option<Uuid>,
    },
    Ended,
}

#[cfg(feature = "session-list")]
impl SessionRecord {
    #[must_use]
    pub fn is_live(self, now: sofie_core::Instant) -> bool {
        match self {
            Self::Unrecorded => true,
            Self::Live { expires_at, .. } => expires_at.is_none_or(|at| at > now),
            Self::Ended => false,
        }
    }
}

/// Changes reach the cookie only once the handler's outcome has committed.
pub struct Session {
    changes: Arc<Mutex<Changes>>,
    current: Option<Uuid>,
    #[cfg(feature = "flows")]
    flow: Option<String>,
    #[cfg(feature = "client-headers")]
    client: Client,
}

/// What a client says of itself: untrusted, read only by handlers that ask.
#[cfg(feature = "client-headers")]
#[derive(Clone, Default)]
struct Client {
    user_agent: Option<HeaderValue>,
    origin: Option<HeaderValue>,
}

#[cfg(feature = "client-headers")]
impl Client {
    fn of(request: &http::request::Parts) -> Self {
        let origin = request.headers.get(http::header::ORIGIN).filter(|origin| {
            crate::csrf::host_of(request)
                .is_some_and(|host| crate::csrf::is_origin_of(origin, host))
        });
        Self {
            user_agent: request.headers.get(http::header::USER_AGENT).cloned(),
            origin: origin.cloned(),
        }
    }
}

#[derive(Default)]
struct Changes {
    requested: Option<Change>,
    #[cfg(feature = "flows")]
    begun_flow: Option<String>,
    committed: bool,
}

#[derive(Clone, Copy)]
enum Change {
    SignIn {
        session: Uuid,
        area: &'static str,
        principal: Uuid,
        /// A session signed back in keeps when it first signed in; a new one signs in now.
        issued_at: Option<Timestamp>,
    },
    SignOut,
}

/// Implemented by a private type of each package whose job is signing principals in, such as
/// sofie-identity's: it alone mints the `SignInRight` that `Session::sign_in` takes. Code that
/// implements it for a type of its own can sign anyone in, so only such a package should.
pub trait Admitter {}

/// The right to sign a principal in, minted only for an `Admitter`; zero-sized, so carrying it
/// costs nothing. It cannot be built any other way:
///
/// ```compile_fail,E0451
/// let right = sofie_http::SignInRight { sealed: () };
/// ```
///
/// ```compile_fail,E0277
/// struct Visitor;
/// let right = sofie_http::SignInRight::of::<Visitor>();
/// ```
#[derive(Clone, Copy)]
pub struct SignInRight {
    sealed: (),
}

const _: () = assert!(size_of::<SignInRight>() == 0);

impl SignInRight {
    #[must_use]
    pub const fn of<K: Admitter>() -> Self {
        Self { sealed: () }
    }
}

impl Session {
    /// Issues a fresh cookie, so no session value from before signing in stays valid; whether
    /// the principal may enter the area is the caller's to check. An action without a
    /// `SignInRight` cannot sign anyone in:
    ///
    /// ```compile_fail,E0061
    /// # use sofie_http::{Area, PrincipalRecord, Session};
    /// fn sign_in_anyone<A: Area>(session: &Session, anyone: sofie_db::Id<PrincipalRecord<A>>) {
    ///     let _ = session.sign_in::<A>(anyone);
    /// }
    /// ```
    pub fn sign_in<A: Area>(
        &self,
        right: SignInRight,
        principal: Id<PrincipalRecord<A>>,
    ) -> Result<Uuid> {
        let SignInRight { sealed: () } = right;
        let session = Uuid::new_v4()
            .map_err(|error| sofie_core::Error::internal("choosing a session id", error))?;
        #[cfg(feature = "product-events")]
        sofie_telemetry::attach_principal(&tracing::Span::current(), principal.uuid());
        self.request(Change::SignIn {
            session,
            area: A::NAME,
            principal: principal.uuid(),
            issued_at: None,
        });
        Ok(session)
    }

    /// Under the session's own id and first sign-in, so a recent sign-in check does not trust it anew.
    pub fn resume<A: Area>(
        &self,
        right: SignInRight,
        session: Uuid,
        principal: Id<PrincipalRecord<A>>,
        issued_at: Timestamp,
    ) {
        let SignInRight { sealed: () } = right;
        #[cfg(feature = "product-events")]
        sofie_telemetry::attach_principal(&tracing::Span::current(), principal.uuid());
        self.request(Change::SignIn {
            session,
            area: A::NAME,
            principal: principal.uuid(),
            issued_at: Some(issued_at),
        });
    }

    #[must_use]
    pub fn current(&self) -> Option<Uuid> {
        self.current
    }

    /// As the client sent it: untrusted, for showing a session's device to its owner.
    #[cfg(feature = "client-headers")]
    #[must_use]
    pub fn user_agent(&self) -> Option<std::borrow::Cow<'_, str>> {
        let value = self.client.user_agent.as_ref()?;
        Some(String::from_utf8_lossy(value.as_bytes()))
    }

    /// The `Origin` the browser sent, when it names the host the request was sent to.
    #[cfg(feature = "client-headers")]
    #[must_use]
    pub fn origin(&self) -> Option<&str> {
        self.client.origin.as_ref()?.to_str().ok()
    }

    pub fn sign_out(&self) {
        self.request(Change::SignOut);
    }

    /// Ties a flow through another site, such as an OAuth sign-in, to this browser: the
    /// returned value goes into the flow's state, and `flow` reads it back on return.
    #[cfg(feature = "flows")]
    pub fn begin_flow(&self) -> Result<String> {
        let value = Uuid::new_v4()
            .map_err(|error| sofie_core::Error::internal("choosing a flow's value", error))?
            .to_string();
        unpoisoned::lock(&self.changes).begun_flow = Some(value.clone());
        Ok(value)
    }

    #[cfg(feature = "flows")]
    #[must_use]
    pub fn flow(&self) -> Option<&str> {
        self.flow.as_deref()
    }

    fn request(&self, change: Change) {
        unpoisoned::lock(&self.changes).requested = Some(change);
    }
}

impl<A: Area> Resolve<RequestContext<A>> for Session {
    fn resolve(context: &mut RequestContext<A>) -> impl Future<Output = Result<Self>> + Send {
        let session = context.session();
        std::future::ready(Ok(Self {
            changes: Arc::clone(&session.changes),
            current: session.recorded,
            #[cfg(feature = "flows")]
            flow: session.flow.clone(),
            #[cfg(feature = "client-headers")]
            client: session.client.clone(),
        }))
    }
}

/// The session the request's cookie carries, read in a page's loader as in an action.
pub struct CurrentSession {
    id: Option<Uuid>,
    account: Option<Uuid>,
}

impl CurrentSession {
    #[must_use]
    pub fn id(&self) -> Option<Uuid> {
        self.id
    }

    /// The account its record signed in, as confirmed live for this request; `None` when no
    /// record was confirmed.
    #[must_use]
    pub fn account(&self) -> Option<Uuid> {
        self.account
    }
}

impl<A: Area, M> Resolve<RequestContext<A, M>> for CurrentSession {
    fn resolve(context: &mut RequestContext<A, M>) -> impl Future<Output = Result<Self>> + Send {
        let session = context.session();
        std::future::ready(Ok(Self {
            id: session.recorded,
            #[cfg(feature = "session-list")]
            account: session.account,
            #[cfg(not(feature = "session-list"))]
            account: None,
        }))
    }
}

pub(crate) struct Sessions {
    pub(crate) keys: Arc<SigningKeys>,
    #[cfg(any(feature = "tokens", feature = "session-list"))]
    pub(crate) members: Option<Arc<dyn AreaMembers>>,
    pub(crate) cookies: CookieNames,
    #[cfg(feature = "session-list")]
    pub(crate) records: crate::session_cache::SessionCache,
}

impl Sessions {
    pub(crate) fn open(&self, headers: &HeaderMap, now: Timestamp) -> RequestSession {
        let sealed = self.cookies.session_value(headers);
        let claims = sealed.and_then(|sealed| opened(&self.keys, sealed, now));
        let is_dead = sealed.is_some() && claims.is_none();
        let recorded = claims.as_ref().map(SessionClaims::session);
        #[cfg(all(feature = "toolbar", debug_assertions))]
        let toolbar = crate::toolbar::Overrides::of(headers, self.cookies.toolbar());
        #[cfg(all(feature = "toolbar", debug_assertions))]
        let (claims, recorded) = match toolbar.claims(now) {
            Some(acting) => (Some(acting), None),
            None => (claims, recorded),
        };
        RequestSession {
            claims,
            recorded,
            #[cfg(feature = "flows")]
            flow: self.cookies.flow_value(headers).map(str::to_owned),
            is_dead,
            #[cfg(all(feature = "toolbar", debug_assertions))]
            toolbar,
            ..RequestSession::default()
        }
    }
}

fn opened(keys: &SigningKeys, sealed: &str, now: Timestamp) -> Option<SessionClaims> {
    SessionClaims::open(keys, sealed, now)
        .map_err(log_unopened)
        .ok()
}

/// A client can send any cookie, so only a corrupt one, which our own keys sealed, is an error.
fn log_unopened(error: OpenError) {
    let refused = match error {
        OpenError::Malformed => "malformed",
        OpenError::Unauthenticated => "unauthenticated",
        OpenError::Expired => "expired",
        OpenError::Corrupt => "corrupt",
    };
    match error {
        OpenError::Malformed | OpenError::Expired => {
            tracing::debug!(session.refused = refused, "the session cookie did not open");
        }
        OpenError::Unauthenticated => {
            tracing::info!(session.refused = refused, "the session cookie did not open");
        }
        OpenError::Corrupt => {
            tracing::error!(error.kind = "session", session.refused = refused, error = %error, "the session cookie did not open");
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct RequestSession {
    claims: Option<SessionClaims>,
    /// The cookie's session, when the claims are the cookie's own.
    recorded: Option<Uuid>,
    /// The account the recorded session signed in, once its record is confirmed live.
    #[cfg(feature = "session-list")]
    account: Option<Uuid>,
    #[cfg(feature = "flows")]
    flow: Option<String>,
    #[cfg(feature = "client-headers")]
    client: Client,
    /// Tampered, sealed by a retired key or expired; it is cleared.
    is_dead: bool,
    changes: Arc<Mutex<Changes>>,
    /// A bearer token's; its request never writes a cookie.
    #[cfg(feature = "tokens")]
    scopes: Option<Arc<[String]>>,
    /// Its principal, when acting as one, has replaced the signed-in one.
    #[cfg(all(feature = "toolbar", debug_assertions))]
    pub(crate) toolbar: crate::toolbar::Overrides,
}

impl RequestSession {
    #[cfg(feature = "tokens")]
    pub(crate) fn bearer(area: &str, bearer: crate::Bearer, now: Timestamp) -> Self {
        let claims = SessionClaims::new(bearer.token, area, bearer.principal, now);
        Self {
            claims: Some(claims),
            scopes: Some(bearer.scopes.into()),
            ..Self::default()
        }
    }

    #[cfg(feature = "engine")]
    pub(crate) fn for_loader(&self) -> Self {
        Self {
            changes: Arc::default(),
            ..self.clone()
        }
    }

    #[cfg(feature = "client-headers")]
    pub(crate) fn with_client(self, request: &http::request::Parts) -> Self {
        Self {
            client: Client::of(request),
            ..self
        }
    }

    #[cfg(feature = "session-list")]
    pub(crate) async fn confirmed(
        mut self,
        sessions: &Sessions,
        db: &sofie_db::Db,
        now: Timestamp,
    ) -> Result<Self> {
        let (Some(session), Some(members)) = (self.recorded, sessions.members.as_deref()) else {
            return Ok(self);
        };
        let seen = self
            .claims
            .as_ref()
            .is_some_and(|claims| claims.needs_refresh(now))
            .then_some(now);
        match sessions.records.live(members, db, session, seen).await? {
            Some(SessionRecord::Live {
                account,
                impersonator,
                ..
            }) => {
                self.account = Some(account);
                if let Some(impersonator) = impersonator {
                    tracing::info!(
                        impersonation.account = %account,
                        impersonation.impersonator = %impersonator,
                        "a request made while impersonating"
                    );
                }
            }
            Some(_) => {}
            None => {
                self.claims = None;
                self.recorded = None;
                self.is_dead = true;
            }
        }
        Ok(self)
    }

    #[cfg(feature = "tokens")]
    pub(crate) fn scopes(&self) -> Option<Arc<[String]>> {
        self.scopes.clone()
    }

    #[cfg_attr(
        not(feature = "tokens"),
        expect(clippy::unused_self, reason = "only a token opens a bearer session")
    )]
    pub(crate) fn is_bearer(&self) -> bool {
        #[cfg(feature = "tokens")]
        return self.scopes.is_some();
        #[cfg(not(feature = "tokens"))]
        false
    }

    pub(crate) fn principal(&self) -> Option<Uuid> {
        self.claims.as_ref().map(SessionClaims::principal)
    }

    #[cfg(feature = "remote-contracts")]
    pub(crate) fn acting(&self) -> Option<sofie_auth::Acting> {
        self.claims.as_ref().map(|claims| sofie_auth::Acting {
            area: claims.area().to_owned(),
            principal: claims.principal(),
        })
    }

    pub(crate) fn principal_in(&self, area: &str) -> Option<Uuid> {
        self.claims
            .as_ref()
            .filter(|claims| claims.area() == area)
            .map(SessionClaims::principal)
    }

    pub(crate) fn committed(&self) {
        unpoisoned::lock(&self.changes).committed = true;
    }

    pub(crate) fn signed_in_area(&self) -> Option<&'static str> {
        match self.committed_change() {
            Some(Change::SignIn { area, .. }) => Some(area),
            _ => None,
        }
    }

    #[cfg(feature = "flows")]
    fn begun_flow(&self) -> Option<String> {
        let changes = unpoisoned::lock(&self.changes);
        changes.begun_flow.clone().filter(|_| changes.committed)
    }

    fn committed_change(&self) -> Option<Change> {
        let changes = unpoisoned::lock(&self.changes);
        changes.requested.filter(|_| changes.committed)
    }

    pub(crate) fn finish(
        &self,
        mut response: Response,
        sessions: &Sessions,
        now: Timestamp,
        caller: Caller<'_>,
    ) -> Response {
        if self.is_bearer() {
            return response;
        }
        #[cfg(feature = "flows")]
        if let Some(flow) = self.begun_flow() {
            match sessions.cookies.begun_flow(&flow) {
                Ok(cookie) => response.headers_mut().append(SET_COOKIE, cookie),
                Err(error) => return answer::failure(error, caller),
            };
        }
        match self.cookie(sessions, now) {
            Ok(Some(cookie)) => {
                response.headers_mut().append(SET_COOKIE, cookie);
                response
            }
            Ok(None) => response,
            Err(error) => answer::failure(error, caller),
        }
    }

    fn cookie(&self, sessions: &Sessions, now: Timestamp) -> Result<Option<HeaderValue>> {
        let (keys, cookies) = (&sessions.keys, &sessions.cookies);
        match self.committed_change() {
            Some(Change::SignIn {
                session,
                area,
                principal,
                issued_at,
            }) => {
                let claims = SessionClaims::new(session, area, principal, now)
                    .issued_at(issued_at.unwrap_or(now));
                cookies.sealed_session(keys, &claims, now).map(Some)
            }
            Some(Change::SignOut) => cookies.session_removal().map(Some),
            None => self.refreshed_cookie(sessions, now),
        }
    }

    fn refreshed_cookie(&self, sessions: &Sessions, now: Timestamp) -> Result<Option<HeaderValue>> {
        let cookies = &sessions.cookies;
        match &self.claims {
            Some(claims) if claims.needs_refresh(now) => cookies
                .sealed_session(&sessions.keys, &claims.clone().seen_at(now), now)
                .map(Some),
            None if self.is_dead => cookies.session_removal().map(Some),
            Some(_) | None => Ok(None),
        }
    }
}
