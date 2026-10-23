use sofie::auth::BearerToken;
use sofie::core::Instant;
use sofie::db::{Outcome, Plan, Reply, Stamp};
use sofie::http::Area;
use sofie::prelude::*;

use super::{AccessToken, AccessTokenId};
use crate::AccountId;
use crate::membership::held;
use crate::role_grant::factory::refuse_unless_permission;

/// Committed as its token; its reply holds the token's text, which is never shown again.
pub struct Issued {
    token: AccessToken,
    secret: BearerToken,
}

impl Issued {
    #[must_use]
    pub fn token(&self) -> &AccessToken {
        &self.token
    }

    #[must_use]
    pub fn secret(&self) -> &BearerToken {
        &self.secret
    }
}

impl Outcome for Issued {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.token.plan(plan)
    }

    fn committed(&mut self, stamp: Stamp) {
        self.token.committed(stamp);
    }

    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(sofie::serde_json::json!({
            "id": self.token.id().uuid().to_string(),
            "token": self.secret.expose(),
        })))
    }
}

/// `scopes` narrow what `account` is granted in a `Permitted` area `A`.
pub async fn issue<A: Area>(
    read: &Read,
    account: AccountId,
    scopes: &[&str],
    expires_at: Instant,
) -> Result<Issued> {
    for scope in scopes {
        refuse_unless_permission("scopes", scope)?;
    }
    if held(read, account, A::NAME).await?.is_none() {
        return Err(Error::Rule(format!(
            "a token for area `{}` needs its account to be a member there",
            A::NAME
        )));
    }
    let secret = BearerToken::generate()?;
    let token = AccessToken {
        meta: Meta::new()?,
        account,
        area: A::NAME.to_owned(),
        digest: secret.digest(),
        scopes: scopes.join(" "),
        expires_at,
        last_used_at: None,
    };
    Ok(Issued { token, secret })
}

pub async fn revoke(read: &Read, account: AccountId, token: AccessTokenId) -> Result<AccessToken> {
    let mut held = AccessToken::query()
        .where_(AccessToken::ID.eq(token))
        .where_(AccessToken::ACCOUNT.eq(account))
        .first(read)
        .await?
        .ok_or(Error::NotFound)?;
    held.delete();
    Ok(held)
}

/// Revoked tokens are left out, expired ones kept until revoked.
pub async fn list(read: &Read, account: AccountId) -> Result<Vec<AccessToken>> {
    AccessToken::query()
        .where_(AccessToken::ACCOUNT.eq(account))
        .latest()
        .all(read)
        .await
}
