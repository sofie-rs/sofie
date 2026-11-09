use sofie::db::{Outcome, Plan};
use sofie::http::{Routes, Shipped};
use sofie::prelude::*;
use sofie::values::Locale;

use super::AccountSession;
use crate::{Account, AccountArea, AccountId, Unscoped, account_of};

/// The account chooses the language of its pages and mails; none follows the browser's.
pub fn language_routes<A: AccountArea>(routes: &mut Routes<A>) {
    let describe: sofie::http::Describe =
        |screens, key| screens.describe_form::<Choosing>(key, "choose-language");
    routes
        .shipped(
            "/language",
            "language",
            (include_str!("language.html"), Some(describe)),
            chosen::<A>,
        )
        .name("language_page");
    routes
        .post("/language", choose::<A>)
        .name("choose_language")
        .then_screen("language", "language");
}

#[derive(ReadModel)]
struct Chosen {
    locale: Option<Locale>,
}

/// Unscoped, as an area's principal may act for the account rather than be it.
async fn chosen<A: AccountArea>(principal: Principal<A>, unscoped: Unscoped) -> Result<Chosen> {
    let account = stored(&unscoped, account_of(&principal)).await?;
    Ok(Chosen {
        locale: account.locale().cloned(),
    })
}

#[derive(Input)]
struct Choosing {
    locale: Option<Locale>,
}

async fn choose<A: AccountArea>(
    principal: Principal<A>,
    choosing: Choosing,
    unscoped: Unscoped,
) -> Result<()> {
    let id = account_of(&principal);
    let mut account = stored(&unscoped, id).await?;
    account.choose_locale(choosing.locale);
    unscoped.0.commit(&mut (account, SessionsRenewed(id))).await
}

async fn stored(unscoped: &Unscoped, id: AccountId) -> Result<Account> {
    Account::query()
        .where_(Account::ID.eq(id))
        .first(&unscoped.0.read())
        .await?
        .ok_or(Error::NotFound)
}

/// Cached session records carry the language, so the account's are read again.
struct SessionsRenewed(AccountId);

impl Outcome for SessionsRenewed {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        plan.touched::<AccountSession>(Some(self.0.uuid()));
        Ok(())
    }
}
