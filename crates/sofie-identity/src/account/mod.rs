pub(crate) mod factory;

use sofie::core::Instant;
use sofie::db::{Outcome, Plan, Stamp};
use sofie::prelude::*;

use crate::PasswordHash;

pub(crate) const EMAIL_INDEX: &str = "account_email";
pub(crate) const ALREADY_REGISTERED: &str = "is already registered";
#[cfg(any(feature = "account-data", feature = "oauth", feature = "impersonation"))]
const CLOSED_DOMAIN: &str = "@closed.invalid";

#[derive(Aggregate)]
#[owner]
pub struct Account {
    meta: Meta,
    email: Email,
    password_hash: PasswordHash,
    verified_at: Option<Instant>,
}

impl Account {
    #[cfg(any(
        feature = "links",
        feature = "totp",
        feature = "passkeys",
        feature = "account-data",
        feature = "impersonation",
        feature = "oauth",
        feature = "permissions"
    ))]
    #[cfg_attr(
        not(any(
            feature = "password-reset",
            feature = "email-verification",
            feature = "sign-in-links",
            feature = "totp",
            feature = "passkeys",
            feature = "account-data",
            feature = "impersonation",
            feature = "oauth",
            feature = "permissions"
        )),
        expect(
            dead_code,
            reason = "with `links` alone, only the unused mailed links call it"
        )
    )]
    pub(crate) async fn stored(read: &Read, id: crate::AccountId) -> Result<Self> {
        Self::query()
            .where_(Self::ID.eq(id))
            .first(read)
            .await?
            .ok_or(Error::NotFound)
    }

    pub(crate) async fn of_email(read: &Read, email: Email) -> Result<Option<Self>> {
        Self::query()
            .where_(Self::EMAIL.eq(email))
            .first(read)
            .await
    }

    #[cfg(any(feature = "oauth", feature = "impersonation"))]
    pub(crate) fn is_closed(&self) -> bool {
        self.email.as_str().ends_with(CLOSED_DOMAIN)
    }

    #[must_use]
    pub fn email(&self) -> &Email {
        &self.email
    }

    #[must_use]
    pub fn verified_at(&self) -> Option<Instant> {
        self.verified_at
    }

    pub(crate) fn password_hash(&self) -> &PasswordHash {
        &self.password_hash
    }

    pub(crate) fn change_password(&mut self, hash: PasswordHash) {
        self.password_hash = hash;
    }

    pub fn confirm_email(&mut self, now: Instant) {
        self.verified_at.get_or_insert(now);
    }

    /// Its first confirmation link is mailed once it is committed.
    #[cfg(feature = "email-verification")]
    pub(crate) fn asking_verification<A: crate::MemberArea>(mut self) -> Result<Self> {
        let purpose = crate::account_link::LinkPurpose::EmailVerification;
        let asked = crate::mailed::LinkAsked::of::<A>(self.email.clone(), purpose);
        self.meta.raise(asked)?;
        Ok(self)
    }

    /// Its address becomes a placeholder no mail reaches and its password matches nothing,
    /// so the account names nobody and signs nobody in.
    #[cfg(feature = "account-data")]
    pub(crate) fn close(&mut self) -> Result<()> {
        self.email = Email::parse(&format!("closed-{}{CLOSED_DOMAIN}", self.id().uuid()))?;
        self.password_hash = PasswordHash::unmatchable()?;
        self.verified_at = None;
        Ok(())
    }

    #[cfg(feature = "account-data")]
    pub(crate) fn raise<E: sofie::db::Event>(&mut self, event: E) -> Result<()> {
        self.meta.raise(event)
    }
}

pub trait Closable: Send {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()>;
    fn committed(&mut self, stamp: Stamp);
}

impl<T: Outcome> Closable for T {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        Outcome::plan(self, plan)
    }

    fn committed(&mut self, stamp: Stamp) {
        Outcome::committed(self, stamp);
    }
}

/// Every way in that whoever held an address before anyone proved it may have set up, ended.
pub(crate) struct Distrusted(Vec<Box<dyn Closable>>);

impl Outcome for Distrusted {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.0.iter().try_for_each(|record| record.plan(plan))
    }

    fn committed(&mut self, stamp: Stamp) {
        for record in &mut self.0 {
            record.committed(stamp);
        }
    }
}

impl Account {
    pub(crate) fn is_confirmed(&self) -> bool {
        self.verified_at.is_some()
    }

    #[cfg(any(feature = "password-reset", feature = "sign-in-links"))]
    pub(crate) async fn prove_email(
        &mut self,
        read: &Read,
        now: Instant,
    ) -> Result<Option<Distrusted>> {
        if self.is_confirmed() {
            return Ok(None);
        }
        self.confirm_email(now);
        self.distrust(read, now).await.map(Some)
    }

    /// The password, sessions and tokens an address's first holder knows stay; only what it
    /// added to sign in without them goes, as confirming says nothing about who added it.
    #[cfg(feature = "email-verification")]
    pub(crate) async fn confirm_unproven(
        &mut self,
        read: &Read,
        now: Instant,
    ) -> Result<Option<Distrusted>> {
        if self.is_confirmed() {
            return Ok(None);
        }
        self.confirm_email(now);
        let records = self.added_ways_in(read).await?;
        Ok(Some(Distrusted(records)))
    }

    /// Only a mailed link, a reset or a provider's confirmed address signs in after this.
    #[cfg_attr(not(any(feature = "sessions", feature = "tokens")), expect(unused_mut))]
    pub(crate) async fn distrust(&mut self, read: &Read, now: Instant) -> Result<Distrusted> {
        self.password_hash = PasswordHash::unmatchable()?;
        let id = self.id();
        let mut records = self.added_ways_in(read).await?;
        #[cfg(feature = "sessions")]
        records.push(Box::new(
            crate::sessions::ending_all(read, id, None, now).await?,
        ));
        #[cfg(feature = "tokens")]
        records.push(Box::new(crate::tokens::revoking(read, id).await?));
        #[cfg(not(feature = "sessions"))]
        let _ = now;
        #[cfg(not(any(feature = "tokens", feature = "sessions")))]
        let _ = id;
        Ok(Distrusted(records))
    }

    /// Passkeys, providers and the second factor.
    #[cfg_attr(
        not(any(feature = "passkeys", feature = "oauth", feature = "totp")),
        expect(clippy::unused_async_trait_impl, unused_mut)
    )]
    async fn added_ways_in(&self, read: &Read) -> Result<Vec<Box<dyn Closable>>> {
        let id = self.id();
        let mut records: Vec<Box<dyn Closable>> = Vec::new();
        #[cfg(feature = "passkeys")]
        records.push(Box::new(crate::passkeys::removing(read, id).await?));
        #[cfg(feature = "oauth")]
        records.push(Box::new(crate::oauth::removing(read, id).await?));
        #[cfg(feature = "totp")]
        records.push(Box::new(crate::two_factor::removing(read, id).await?));
        #[cfg(not(any(feature = "passkeys", feature = "oauth", feature = "totp")))]
        let _ = (read, id);
        Ok(records)
    }
}
