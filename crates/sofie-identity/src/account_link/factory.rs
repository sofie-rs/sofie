use std::time::Duration;

use sofie::core::Timestamp;
use sofie::prelude::*;

use super::{AccountLink, LinkPurpose};
use crate::AccountId;

/// Long enough to find the mail, short enough that an old one in a mailbox opens nothing.
const VALID_FOR: Duration = Duration::from_hours(1);
/// Signing in opens the account outright, so its link is followed at once or not at all.
const SIGN_IN_VALID_FOR: Duration = Duration::from_mins(15);
/// The code is at hand while the password was just typed.
const SECOND_FACTOR_VALID_FOR: Duration = Duration::from_mins(5);

/// Its expiry falls on a whole second, as the signed link's does, so both name one moment.
pub(crate) fn issue(account: AccountId, purpose: LinkPurpose, now: Instant) -> Result<AccountLink> {
    let valid_for = match purpose {
        LinkPurpose::SignIn => SIGN_IN_VALID_FOR,
        LinkPurpose::SecondFactor => SECOND_FACTOR_VALID_FOR,
        LinkPurpose::PasswordReset | LinkPurpose::EmailVerification | LinkPurpose::Join => {
            VALID_FOR
        }
    };
    let expires = now.checked_add(valid_for)?.timestamp().as_second();
    let expires_at = Timestamp::from_second(expires)
        .map_err(|error| Error::internal("computing a link's expiry", error))?;
    Ok(AccountLink {
        meta: Meta::new()?,
        account,
        purpose,
        expires_at: Instant::from(expires_at),
        used_at: None,
    })
}
