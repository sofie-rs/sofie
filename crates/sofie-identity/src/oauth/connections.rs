use sofie::core::Instant;
use sofie::http::{Routes, Sealing, Shipped};
use sofie::prelude::*;

use super::sign_in::{Chosen, Location};
use super::{AccountIdentity, Installed, OAuth, flow};
use crate::pages::Choice;
use crate::signing_in::{OwnSession, RecentSignIn};
use crate::{MemberArea, account_of};

pub(crate) const PATH: &str = "/connections";

/// Each member connects and disconnects only its own account's providers.
pub fn routes<A: MemberArea>(routes: &mut Routes<A>) {
    routes
        .shipped(
            PATH,
            "connections",
            (include_str!("connections.html"), None),
            list::<A>,
        )
        .name("list_connections");
    routes
        .post(PATH, connect::<A>)
        .name("connect_provider")
        .then_follow();
    routes
        .post(
            &format!("{PATH}/{{account_identity}}/remove"),
            disconnect::<A>,
        )
        .name("disconnect_provider")
        .then_screen("connections", "connections");
}

#[derive(ReadModel)]
#[read(from = AccountIdentity)]
struct Connected {
    id: Id<AccountIdentity>,
    provider: String,
    email: Option<Email>,
    created_at: Instant,
}

#[derive(ReadModel)]
pub struct Connections {
    connected: Vec<Connected>,
    providers: Vec<Choice>,
}

async fn list<A: MemberArea>(
    principal: Principal<A>,
    read: Read,
    installed: Installed,
) -> Result<Connections> {
    let connected = Connected::query()
        .where_(AccountIdentity::ACCOUNT.eq(account_of(&principal)))
        .latest()
        .all(&read)
        .await?;
    let providers = installed.0.as_deref().map_or_else(Vec::new, OAuth::choices);
    Ok(Connections {
        connected,
        providers,
    })
}

async fn connect<A: MemberArea>(
    principal: Principal<A>,
    _recent: RecentSignIn,
    chosen: Chosen,
    installed: Installed,
    session: Session,
    sealing: Sealing,
    clock: Clock,
) -> Result<Location> {
    let link = Some(account_of(&principal).uuid());
    let location = flow::begin::<A>(
        installed.get()?,
        &chosen.provider,
        &session,
        &sealing,
        clock.now(),
        link,
    )?;
    Ok(Location { location })
}

/// Checked here too, since a `Permitted` area may reach records it does not own.
async fn disconnect<A: MemberArea>(
    principal: Principal<A>,
    _own: OwnSession,
    mut identity: AccountIdentity,
) -> Result<AccountIdentity> {
    if identity.account != account_of(&principal) {
        return Err(Error::NotFound);
    }
    identity.delete();
    Ok(identity)
}
