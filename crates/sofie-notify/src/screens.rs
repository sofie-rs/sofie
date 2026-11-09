use serde::Serialize;
use sofie_core::Result;
use sofie_db::{Outcome, ReadOnly, Reply};
use sofie_http::{Area, Routes, Shipped};

use crate::{Inbox, InboxItem, MarkedRead};

const SHOWN: u64 = 50;

/// The principal's inbox at `/notifications`, refreshed as notifications arrive.
pub fn inbox_routes<A: Area>(routes: &mut Routes<A>) {
    routes
        .shipped(
            "/notifications",
            "inbox",
            (include_str!("screens/inbox.html"), None),
            listed,
        )
        .name("inbox")
        .live("inbox");
    routes
        .post("/notifications/read", read_all)
        .name("mark_inbox_read")
        .then_screen("inbox", "inbox");
}

#[derive(Serialize)]
pub struct Listed {
    items: Vec<InboxItem>,
    any_unread: bool,
}

impl Outcome for Listed {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Data(Box::new(self)))
    }
}

impl ReadOnly for Listed {}

async fn listed(inbox: Inbox) -> Result<Listed> {
    let items = inbox.latest(SHOWN).await?;
    Ok(Listed {
        any_unread: inbox.unread().await? > 0,
        items,
    })
}

async fn read_all(inbox: Inbox) -> Result<MarkedRead> {
    Ok(inbox.mark_all_read())
}
