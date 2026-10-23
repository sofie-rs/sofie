use sofie::http::{Routes, Web};
use sofie::prelude::*;

use super::{Mailed, link_routes};
use crate::account::{Account, Distrusted};
use crate::account_link::{AccountLink, LinkPurpose};
use crate::{MemberArea, Password, PasswordHash};

struct Reset;

impl Mailed for Reset {
    const PURPOSE: LinkPurpose = LinkPurpose::PasswordReset;
    const ASKING: &'static str = "/forgot-password";
    const SCREENS: [&'static str; 2] = ["forgot_password", "reset_password"];
    const NAMES: [&'static str; 3] = ["forgot_page", "ask_reset", "reset_page"];
}

#[derive(Input)]
pub struct NewPassword {
    new_password: Password,
}

pub(super) fn routes<A: MemberArea>(routes: &mut Routes<Web>) {
    let reset = link_routes::<A, Reset>(routes);
    routes
        .post(&reset, reset_password)
        .name_for::<A>("reset_password")
        .signed()
        .then_redirect(A::SIGN_IN);
}

#[cfg(feature = "sessions")]
type Ended = (
    Vec<crate::sessions::AccountSession>,
    crate::sessions::Impersonations,
);
#[cfg(not(feature = "sessions"))]
type Ended = ();

#[cfg(feature = "tokens")]
type Revoked = Vec<crate::tokens::AccessToken>;
#[cfg(not(feature = "tokens"))]
type Revoked = ();

/// Uses up every other unused link of the account too, so no older mail opens it, and
/// revokes its access tokens, which a thief of the old password may have issued.
/// With `sessions`, every session of the account ends; without, sessions live only in
/// their cookies, so ones already open elsewhere stay open. A reset proving the address
/// for the first time distrusts the account before setting the password, as a sign-in link does.
async fn reset_password(
    mut link: AccountLink,
    chosen: NewPassword,
    read: Read,
    clock: Clock,
) -> Result<(
    Account,
    Vec<AccountLink>,
    Option<Distrusted>,
    Option<(Ended, Revoked)>,
)> {
    let now = clock.now();
    link.use_up(Reset::PURPOSE, now)?;
    let mut account = Account::stored(&read, link.account()).await?;
    let distrusted = account.prove_email(&read, now).await?;
    account.change_password(PasswordHash::new(&chosen.new_password).await?);
    let mut others = AccountLink::query()
        .where_(AccountLink::ACCOUNT.eq(link.account()))
        .where_(AccountLink::USED_AT.is_null())
        .all(&read)
        .await?;
    others.retain(|other| other.id() != link.id());
    for other in &mut others {
        other.retire(now);
    }
    others.push(link);
    let signed_out = match distrusted {
        Some(_) => None,
        None => Some(signed_out(&read, account.id(), now).await?),
    };
    Ok((account, others, distrusted, signed_out))
}

#[cfg_attr(
    not(any(feature = "sessions", feature = "tokens")),
    expect(clippy::unused_async)
)]
async fn signed_out(
    read: &Read,
    account: crate::AccountId,
    now: sofie::core::Instant,
) -> Result<(Ended, Revoked)> {
    #[cfg(feature = "sessions")]
    let ended = crate::sessions::ending_everywhere(read, account, now).await?;
    #[cfg(not(feature = "sessions"))]
    let ended = ();
    #[cfg(feature = "tokens")]
    let revoked = crate::tokens::revoking(read, account).await?;
    #[cfg(not(feature = "tokens"))]
    let revoked = ();
    #[cfg(not(feature = "sessions"))]
    let _ = now;
    #[cfg(not(any(feature = "sessions", feature = "tokens")))]
    let _ = (read, account);
    Ok((ended, revoked))
}
