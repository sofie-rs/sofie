use sofie::core::Instant;
use sofie::http::{Routes, Shipped};
use sofie::prelude::*;

use super::{AccountSession, Impersonations, ending_all, impersonations_in};
use crate::signing_in::OwnSession;
use crate::{AccountArea, Unscoped, account_of};

/// Each principal sees and ends only its own account's sessions.
pub fn routes<A: AccountArea>(routes: &mut Routes<A>) {
    routes
        .shipped(
            "/sessions",
            "sessions",
            (include_str!("screen.html"), None),
            own::<A>,
        )
        .name("list_sessions");
    routes
        .post("/sessions/{account_session}/end", end_one::<A>)
        .name("end_session")
        .then_screen("sessions", "sessions");
    routes
        .post("/sessions/others", end_others::<A>)
        .name("end_other_sessions")
        .then_screen("sessions", "sessions");
}

#[derive(ReadModel)]
#[read(from = AccountSession)]
struct Live {
    id: Id<AccountSession>,
    area: String,
    device: Option<String>,
    last_seen_at: Instant,
    created_at: Instant,
}

async fn own<A: AccountArea>(principal: Principal<A>, read: Read) -> Result<Vec<Live>> {
    Live::query()
        .where_(AccountSession::ACCOUNT.eq(account_of(&principal)))
        .where_(AccountSession::ENDED_AT.is_null())
        .latest()
        .all(&read)
        .await
}

/// Checked here too, since a `Permitted` area may reach records it does not own.
async fn end_one<A: AccountArea>(
    principal: Principal<A>,
    _own: OwnSession,
    mut record: AccountSession,
    read: Read,
    clock: Clock,
) -> Result<(AccountSession, Impersonations)> {
    if record.account != account_of(&principal) {
        return Err(Error::NotFound);
    }
    let now = clock.now();
    record.end(now);
    let impersonations = impersonations_in(&read, std::slice::from_ref(&record), now).await?;
    Ok((record, impersonations))
}

/// Unscoped, as the sessions of the impersonations the account started are its members'.
async fn end_others<A: AccountArea>(
    principal: Principal<A>,
    _own: OwnSession,
    session: Session,
    unscoped: Unscoped,
    clock: Clock,
) -> Result<()> {
    let read = unscoped.0.read();
    let mut ended = ending_all(
        &read,
        account_of(&principal),
        session.current(),
        clock.now(),
    )
    .await?;
    unscoped.0.commit(&mut ended).await
}
