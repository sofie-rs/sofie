use sofie::core::{Instant, Timestamp};
use sofie::db::{Db, Uuid};
use sofie::http::SignIns;
use sofie::prelude::*;

use crate::account::Account;
use crate::notice::Notice;

/// An app listens with `#[on]` to tell the owner.
#[derive(Event)]
pub struct SignInLocked {
    pub account: Uuid,
    pub until: Instant,
}

/// While a lock holds, every attempt reads as a wrong password, so no answer tells a guess right.
pub(crate) fn admits(sign_ins: &SignIns, account: Option<&Account>, matches: bool) -> bool {
    let account = account.map(|account| account.id().uuid());
    if account.is_some_and(|account| sign_ins.locked(account)) {
        return false;
    }
    if !matches {
        sign_ins.failed(account);
    }
    matches
}

pub(crate) async fn tell_owner(db: &Db, account: Uuid, until: Timestamp) -> Result<()> {
    let locked = SignInLocked {
        account,
        until: Instant::from(until),
    };
    db.commit(&mut Notice::of(locked)?).await
}
