mod factory;
#[cfg(feature = "token-screens")]
pub mod screen;

use std::time::Duration;

use sofie::core::Instant;
use sofie::db::{Db, in_select};
use sofie::http::Bearer;
use sofie::prelude::*;

pub use factory::{Issued, issue, list, revoke};

use crate::AccountId;
use crate::membership::Membership;

/// A write per request would cost more than the request itself.
const LAST_USE_PRECISION: Duration = Duration::from_secs(60);

/// Only the digest of its text is stored; revoking deletes it softly.
#[derive(Aggregate)]
#[aggregate(behaviors(SoftDelete))]
pub struct AccessToken {
    meta: Meta,
    #[owner]
    account: AccountId,
    area: String,
    digest: String,
    scopes: String,
    expires_at: Instant,
    last_used_at: Option<Instant>,
}

pub type AccessTokenId = Id<AccessToken>;

impl AccessToken {
    #[must_use]
    pub fn account(&self) -> AccountId {
        self.account
    }

    #[must_use]
    pub fn area(&self) -> &str {
        &self.area
    }

    pub fn scopes(&self) -> impl Iterator<Item = &str> {
        self.scopes.split_whitespace()
    }

    #[must_use]
    pub fn expires_at(&self) -> Instant {
        self.expires_at
    }

    #[must_use]
    pub fn last_used_at(&self) -> Option<Instant> {
        self.last_used_at
    }
}

/// Membership is checked here, not only at issue, so a removed member's token stops at once.
pub(crate) async fn bearer(db: &Db, area: &str, digest: &str) -> Result<Option<Bearer>> {
    let members = Membership::AREA.eq(area.to_owned());
    let found = AccessToken::query()
        .where_(AccessToken::DIGEST.eq(digest.to_owned()))
        .where_(AccessToken::AREA.eq(area.to_owned()))
        .where_(in_select(
            AccessToken::ACCOUNT,
            Membership::ACCOUNT,
            members,
        ))
        .first(&db.read())
        .await?;
    let now = db.clock().now();
    let Some(mut token) = found.filter(|token| token.expires_at > now) else {
        return Ok(None);
    };
    record_use(db, &mut token, now).await?;
    Ok(Some(Bearer {
        token: token.id().uuid(),
        principal: token.account.uuid(),
        scopes: token.scopes().map(str::to_owned).collect(),
    }))
}

async fn record_use(db: &Db, token: &mut AccessToken, now: Instant) -> Result<()> {
    let recent_since = now.checked_sub(LAST_USE_PRECISION)?;
    if token.last_used_at.is_some_and(|used| used > recent_since) {
        return Ok(());
    }
    token.last_used_at = Some(now);
    match db.commit(token).await {
        // A request with the same token recorded its use first.
        Err(Error::Conflict) => Ok(()),
        recorded => recorded,
    }
}

/// What each live token may do and until when; never its digest.
#[cfg(feature = "account-data")]
pub(crate) async fn exported(read: &Read, account: AccountId) -> Result<sofie::serde_json::Value> {
    let tokens = list(read, account).await?;
    Ok(sofie::serde_json::json!(
        tokens
            .iter()
            .map(|token| sofie::serde_json::json!({
                "area": token.area,
                "scopes": token.scopes().collect::<Vec<_>>(),
                "expires_at": token.expires_at,
                "last_used_at": token.last_used_at,
            }))
            .collect::<Vec<_>>()
    ))
}

pub(crate) async fn revoking(read: &Read, account: AccountId) -> Result<Vec<AccessToken>> {
    let mut tokens = list(read, account).await?;
    for token in &mut tokens {
        token.delete();
    }
    Ok(tokens)
}
