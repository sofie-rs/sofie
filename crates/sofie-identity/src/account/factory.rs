use sofie::prelude::*;

use super::Account;
use crate::{Password, PasswordHash};

/// Hashes the password even when the address is taken, so a taken address answers as late as
/// a new one; `None` then. The unique index still refuses a racing sign-up.
pub(crate) async fn register(
    read: &Read,
    email: Email,
    password: &Password,
) -> Result<Option<Account>> {
    let taken = Account::query()
        .where_(Account::EMAIL.eq(email.clone()))
        .exists(read)
        .await?;
    let password_hash = PasswordHash::new(password).await?;
    if taken {
        return Ok(None);
    }
    Ok(Some(Account {
        meta: Meta::new()?,
        email,
        password_hash,
        verified_at: None,
    }))
}

/// An account signed up through a provider or added by a task, which a password reset gives
/// a password.
pub(crate) fn without_password(email: Email) -> Result<Account> {
    Ok(Account {
        meta: Meta::new()?,
        email,
        password_hash: PasswordHash::unmatchable()?,
        verified_at: None,
    })
}
