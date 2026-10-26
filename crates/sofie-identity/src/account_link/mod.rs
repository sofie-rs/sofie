pub(crate) mod factory;

use std::time::Duration;

use sofie::http::{Area, SignedUrls};
use sofie::prelude::*;

use crate::AccountId;
use crate::pages::FormPage;

/// Following it once proves the address is the reader's.
#[derive(Aggregate)]
pub struct AccountLink {
    meta: Meta,
    #[owner]
    account: AccountId,
    purpose: LinkPurpose,
    expires_at: Instant,
    used_at: Option<Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enum)]
pub enum LinkPurpose {
    PasswordReset,
    EmailVerification,
    SignIn,
    SecondFactor,
    Join,
}

impl LinkPurpose {
    pub(crate) const fn segment(self) -> &'static str {
        match self {
            Self::PasswordReset => "/reset-password",
            Self::EmailVerification => "/verify-email",
            Self::SignIn => "/sign-in-link",
            Self::SecondFactor => "/two-factor",
            Self::Join => "/join",
        }
    }
}

/// What the page a link opens reads of it; a GET loads no aggregate.
#[derive(ReadModel)]
#[read(from = AccountLink)]
pub struct LinkPage {
    id: Id<AccountLink>,
    purpose: LinkPurpose,
    expires_at: Instant,
    used_at: Option<Instant>,
}

pub(crate) const PRUNE_EVERY: Duration = Duration::from_hours(1);
const PRUNE_MARGIN: Duration = Duration::from_hours(1);

pub(crate) const INVALID: &str = "This link is invalid, was used already or has expired.";

impl AccountLink {
    pub(crate) fn account(&self) -> AccountId {
        self.account
    }

    pub(crate) fn use_up(&mut self, purpose: LinkPurpose, now: Instant) -> Result<()> {
        if !self.is_open(purpose, now) {
            return Err(Error::Rule(INVALID.to_owned()));
        }
        self.used_at = Some(now);
        Ok(())
    }

    pub(crate) fn is_open(&self, purpose: LinkPurpose, now: Instant) -> bool {
        opens((self.purpose, self.used_at, self.expires_at), purpose, now)
    }

    pub(crate) fn expires_at(&self) -> Instant {
        self.expires_at
    }

    #[cfg(any(feature = "password-reset", feature = "account-data"))]
    pub(crate) fn retire(&mut self, now: Instant) {
        self.used_at = Some(now);
    }
}

fn opens(
    (held, used_at, expires_at): (LinkPurpose, Option<Instant>, Instant),
    purpose: LinkPurpose,
    now: Instant,
) -> bool {
    held == purpose && used_at.is_none() && expires_at > now
}

impl LinkPage {
    fn open_until(&self, purpose: LinkPurpose, now: Instant) -> Result<Instant> {
        if !opens((self.purpose, self.used_at, self.expires_at), purpose, now) {
            return Err(Error::Rule(INVALID.to_owned()));
        }
        Ok(self.expires_at)
    }

    #[cfg(feature = "links")]
    pub(crate) fn form<A: Area>(
        &self,
        purpose: LinkPurpose,
        urls: &SignedUrls,
        clock: &Clock,
    ) -> Result<FormPage> {
        self.form_at(purpose, &self.path::<A>(purpose), urls, clock)
    }

    #[cfg(feature = "totp")]
    pub(crate) fn form_carrying<A: Area>(
        &self,
        purpose: LinkPurpose,
        urls: &SignedUrls,
        clock: &Clock,
        next: &sofie::http::Next,
    ) -> Result<FormPage> {
        let path = next.carried_by(&self.path::<A>(purpose));
        self.form_at(purpose, &path, urls, clock)
    }

    fn path<A: Area>(&self, purpose: LinkPurpose) -> String {
        format!("{}{}/{}", A::PREFIX, purpose.segment(), self.id.uuid())
    }

    fn form_at(
        &self,
        purpose: LinkPurpose,
        path: &str,
        urls: &SignedUrls,
        clock: &Clock,
    ) -> Result<FormPage> {
        let now = clock.now();
        let expires_at = self.open_until(purpose, now)?;
        let action = signed_action(urls, path, expires_at, now)?;
        Ok(FormPage::new(action, None))
    }
}

/// Signed again to the second of the link's own expiry, so a page and the link it came from
/// carry one signature and a refused form shows again at the address it was posted to.
pub(crate) fn signed_action(
    urls: &SignedUrls,
    path: &str,
    expires_at: Instant,
    now: Instant,
) -> Result<String> {
    let left = expires_at.timestamp().duration_since(now.timestamp());
    let left = Duration::try_from(left).map_err(|_| Error::Rule(INVALID.to_owned()))?;
    urls.sign(path, left)
}

/// Kept an hour past its expiry, so the hourly limit on mailed links still counts it.
pub(crate) async fn prune(prune: sofie::db::Prune, clock: Clock) -> Result<()> {
    let before = clock.now().checked_sub(PRUNE_MARGIN)?;
    prune
        .before(AccountLink::EXPIRES_AT, before)
        .await
        .map(drop)
}

/// Used up, so no link mailed before the account closed opens it again.
#[cfg(feature = "account-data")]
pub(crate) async fn closing(
    read: &Read,
    account: AccountId,
    now: Instant,
) -> Result<Vec<AccountLink>> {
    let mut unused = AccountLink::query()
        .where_(AccountLink::ACCOUNT.eq(account))
        .where_(AccountLink::USED_AT.is_null())
        .where_(AccountLink::EXPIRES_AT.gt(now))
        .all(read)
        .await?;
    for link in &mut unused {
        link.retire(now);
    }
    Ok(unused)
}
