use sofie::prelude::*;

use super::Membership;
use crate::AccountId;

pub(crate) fn grant(account: AccountId, area: &str) -> Result<Membership> {
    Ok(Membership {
        meta: Meta::new()?,
        account,
        area: area.to_owned(),
        #[cfg(feature = "permissions")]
        role: None,
    })
}
