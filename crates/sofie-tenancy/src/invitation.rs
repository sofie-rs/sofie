use std::time::Duration;

use sofie::core::Instant;
use sofie::db::{Query, Source};
use sofie::prelude::*;

use sofie_identity::AccountId;

use crate::organization::{OrganizationId, OrganizationRole};

/// Long enough for a colleague back from a week away.
pub(crate) const VALID_FOR: Duration = Duration::from_hours(7 * 24);
pub(crate) const NOT_OPEN: &str = "This invitation was accepted already or has expired.";

/// A role offered to whoever holds the mailed link and signs in with its address.
#[derive(Aggregate)]
pub struct Invitation {
    meta: Meta,
    #[owner]
    organization: OrganizationId,
    email: Email,
    role: OrganizationRole,
    invited_by: AccountId,
    expires_at: Instant,
    accepted_at: Option<Instant>,
}

impl Invitation {
    pub(crate) fn new(
        organization: OrganizationId,
        email: Email,
        role: OrganizationRole,
        invited_by: AccountId,
        now: Instant,
    ) -> Result<Self> {
        Ok(Self {
            meta: Meta::new()?,
            organization,
            email,
            role,
            invited_by,
            expires_at: now.checked_add(VALID_FOR)?,
            accepted_at: None,
        })
    }

    #[must_use]
    pub fn organization(&self) -> OrganizationId {
        self.organization
    }

    #[must_use]
    pub fn email(&self) -> &Email {
        &self.email
    }

    #[must_use]
    pub fn role(&self) -> OrganizationRole {
        self.role
    }

    pub(crate) fn invited_by(&self) -> AccountId {
        self.invited_by
    }

    #[must_use]
    pub fn expires_at(&self) -> Instant {
        self.expires_at
    }

    #[must_use]
    pub fn is_open(&self, now: Instant) -> bool {
        self.accepted_at.is_none() && self.expires_at > now
    }

    /// Expired at once, so its link opens nothing and its seat is free.
    pub(crate) fn withdraw(&mut self, now: Instant) -> Result<()> {
        if !self.is_open(now) {
            return Err(Error::Rule(NOT_OPEN.to_owned()));
        }
        self.expires_at = now;
        Ok(())
    }

    pub(crate) fn accept(&mut self, now: Instant) -> Result<()> {
        if !self.is_open(now) {
            return Err(Error::Rule(NOT_OPEN.to_owned()));
        }
        self.accepted_at = Some(now);
        Ok(())
    }
}

/// Those still waiting, which hold a seat as a member does.
pub(crate) fn pending<T: Source<Base = Invitation>>(
    query: Query<T>,
    organization: OrganizationId,
    now: Instant,
) -> Query<T> {
    query
        .where_(Invitation::ORGANIZATION.eq(organization))
        .where_(Invitation::ACCEPTED_AT.is_null())
        .where_(Invitation::EXPIRES_AT.gt(now))
}
