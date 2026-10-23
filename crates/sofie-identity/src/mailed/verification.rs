use sofie::http::{Routes, Web};
use sofie::prelude::*;

use super::{Mailed, link_routes};
use crate::MemberArea;
use crate::account::{Account, Distrusted};
use crate::account_link::{AccountLink, LinkPurpose};

struct Verification;

impl Mailed for Verification {
    const PURPOSE: LinkPurpose = LinkPurpose::EmailVerification;
    const SCREENS: [&'static str; 2] = ["ask_verification", "verify_email"];
    const NAMES: [&'static str; 3] = ["ask_page", "ask_verification", "verify_page"];
}

pub(super) fn routes<A: MemberArea>(routes: &mut Routes<Web>) {
    let verify = link_routes::<A, Verification>(routes);
    routes
        .post(&verify, verify_email)
        .name_for::<A>("verify_email")
        .signed()
        .then_redirect(A::SIGN_IN);
}

async fn verify_email(
    mut link: AccountLink,
    read: Read,
    clock: Clock,
) -> Result<(Account, AccountLink, Option<Distrusted>)> {
    let now = clock.now();
    link.use_up(Verification::PURPOSE, now)?;
    let mut account = Account::stored(&read, link.account()).await?;
    let distrusted = account.confirm_unproven(&read, now).await?;
    Ok((account, link, distrusted))
}
