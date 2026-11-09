use std::time::Duration;

use sofie::core::Instant;
use sofie::http::{NeedsPermission, Permission, Routes, Sealing, Shipped};
use sofie::prelude::*;

use super::{AccessToken, AccessTokenId, Issued, REVEALING};
use crate::signing_in::OwnSession;
use crate::{AccountArea, Unscoped, account_of};

const READ: Permission = Permission::new("sofie_identity.access_token.*.read");
const REVOKE: Permission = Permission::new("sofie_identity.access_token.*.delete");
const CREATE: Permission = Permission::new("sofie_identity.access_token.*.create");
const DAY: Duration = Duration::from_hours(24);
/// A token's text is shown this long at most; pruning then erases its sealed copy.
const REVEALED_FOR: Duration = Duration::from_mins(10);

/// Each principal sees and revokes only the tokens its own account issued.
pub fn routes<A: AccountArea>(routes: &mut Routes<A>) {
    let describe: sofie::http::Describe =
        |screens, key| screens.describe_form::<Revoking>(key, "revoke");
    routes
        .shipped(
            "/tokens",
            "tokens",
            (
                include_str!("screen.html"),
                include_bytes!(concat!(env!("OUT_DIR"), "/shipped/tokens/screen.sft")),
                Some(describe),
            ),
            own::<A>,
        )
        .name("list_tokens")
        .permission(READ);
    routes
        .post("/tokens", revoke_own::<A>)
        .name("revoke_token")
        .permission(REVOKE)
        .then_screen("tokens", "tokens");
    let describe: sofie::http::Describe =
        |screens, key| screens.describe_form::<Issuing>(key, "issue-token");
    routes
        .shipped(
            "/tokens/new",
            "issue_token",
            (
                include_str!("issue.html"),
                include_bytes!(concat!(env!("OUT_DIR"), "/shipped/tokens/issue.sft")),
                Some(describe),
            ),
            revealed::<A>,
        )
        .name("issue_token_page")
        .permission(CREATE);
    routes
        .post("/tokens/new", issue_own::<A>)
        .name("issue_token")
        .permission(CREATE)
        .then_screen("issue_token", "issued");
    routes
        .post("/tokens/new/done", seen::<A>)
        .name("token_seen")
        .permission(CREATE)
        .then_screen("issue_token", "issued");
}

#[derive(ReadModel)]
#[read(from = AccessToken)]
struct Held {
    id: Id<AccessToken>,
    area: String,
    scopes: String,
    expires_at: Instant,
    last_used_at: Option<Instant>,
}

async fn own<A: AccountArea>(principal: Principal<A>, read: Read) -> Result<Vec<Held>> {
    Held::query()
        .where_(AccessToken::ACCOUNT.eq(account_of(&principal)))
        .latest()
        .all(&read)
        .await
}

#[derive(Input)]
struct Revoking {
    token: AccessTokenId,
}

async fn revoke_own<A: AccountArea>(
    principal: Principal<A>,
    _own: OwnSession,
    read: Read,
    revoking: Revoking,
) -> Result<AccessToken> {
    super::revoke(&read, account_of(&principal), revoking.token).await
}

#[derive(Input)]
struct Issuing {
    /// Permissions the token is narrowed to, separated by spaces; none keeps all of the account's.
    #[max(2000)]
    scopes: Option<String>,
    #[min(1)]
    #[max(365)]
    days: u16,
}

async fn issue_own<A: AccountArea>(
    principal: Principal<A>,
    _own: OwnSession,
    read: Read,
    clock: Clock,
    sealing: Sealing,
    issuing: Issuing,
) -> Result<Issued> {
    let scopes = issuing.scopes.unwrap_or_default();
    let scopes: Vec<&str> = scopes.split_whitespace().collect();
    let expires_at = clock
        .now()
        .checked_add(DAY.saturating_mul(u32::from(issuing.days)))?;
    super::issue::<A>(&read, account_of(&principal), &scopes, expires_at)
        .await?
        .revealed(&sealing)
}

#[derive(ReadModel)]
pub struct Revealing {
    revealed: Option<Revealed>,
}

#[derive(ReadModel)]
struct Revealed {
    token: String,
}

/// An impersonation never sees the member's tokens' text.
async fn revealed<A: AccountArea>(
    principal: Principal<A>,
    _own: OwnSession,
    read: Read,
    clock: Clock,
    sealing: Sealing,
) -> Result<Revealing> {
    let since = clock.now().checked_sub(REVEALED_FOR)?;
    let Some(sealed) = unseen(&read, &principal)
        .await?
        .into_iter()
        .filter(|token| token.created_at().is_some_and(|created| created > since))
        .find_map(|token| token.revealing)
    else {
        return Ok(Revealing { revealed: None });
    };
    let token = sealing
        .open(REVEALING, &sealed)
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| {
            Error::internal_message("a new token's text does not open with the app's keys")
        })?;
    Ok(Revealing {
        revealed: Some(Revealed { token }),
    })
}

async fn seen<A: AccountArea>(
    principal: Principal<A>,
    _own: OwnSession,
    read: Read,
) -> Result<Vec<AccessToken>> {
    let mut unseen = unseen(&read, &principal).await?;
    for token in &mut unseen {
        token.revealing = None;
    }
    Ok(unseen)
}

async fn unseen<A: AccountArea>(read: &Read, principal: &Principal<A>) -> Result<Vec<AccessToken>> {
    AccessToken::query()
        .where_(AccessToken::ACCOUNT.eq(account_of(principal)))
        .where_(AccessToken::REVEALING.is_not_null())
        .latest()
        .all(read)
        .await
}

/// Revoked tokens too, so no sealed text outlives its window.
pub(crate) async fn forget_revealing(unscoped: &Unscoped, clock: &Clock) -> Result<()> {
    let before = clock.now().checked_sub(REVEALED_FOR)?;
    let mut stale = AccessToken::query()
        .with_trashed()
        .where_(AccessToken::REVEALING.is_not_null())
        .where_(AccessToken::CREATED_AT.lt(before))
        .all(&unscoped.0.read())
        .await?;
    for token in &mut stale {
        token.revealing = None;
    }
    match unscoped.0.commit(&mut stale).await {
        // Used or dismissed meanwhile, which cleared it too; the rest goes at the next pruning.
        Err(Error::Conflict) => Ok(()),
        forgotten => forgotten,
    }
}
