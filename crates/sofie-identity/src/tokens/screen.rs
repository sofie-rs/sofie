use sofie::core::Instant;
use sofie::http::{Area, NeedsPermission, Permission, Routes, Shipped};
use sofie::prelude::*;

use super::{AccessToken, AccessTokenId};
use crate::account_of;

const READ: Permission = Permission::new("sofie_identity.access_token.*.read");
const REVOKE: Permission = Permission::new("sofie_identity.access_token.*.delete");

/// Each principal sees and revokes only the tokens its own account issued.
pub fn routes<A: Area>(routes: &mut Routes<A>) {
    let describe: sofie::http::Describe =
        |screens, key| screens.describe_form::<Revoking>(key, "revoke");
    routes
        .shipped(
            "/tokens",
            "tokens",
            (include_str!("screen.html"), Some(describe)),
            own::<A>,
        )
        .name("list_tokens")
        .permission(READ);
    routes
        .post("/tokens", revoke_own::<A>)
        .name("revoke_token")
        .permission(REVOKE)
        .then_screen("tokens", "tokens");
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

async fn own<A: Area>(principal: Principal<A>, read: Read) -> Result<Vec<Held>> {
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

async fn revoke_own<A: Area>(
    principal: Principal<A>,
    read: Read,
    revoking: Revoking,
) -> Result<AccessToken> {
    super::revoke(&read, account_of(&principal), revoking.token).await
}
