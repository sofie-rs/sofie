use sofie::core::Result;
use sofie::db::{Create, Id, Meta, Update};
use sofie::prelude::*;

use super::{Project, Ticket};

#[derive(Input)]
pub struct NewTicket {
    project: Id<Project>,
    #[required]
    title: String,
}

#[derive(Input)]
pub struct TicketChange {
    #[required]
    title: String,
}

impl Create for Ticket {
    type Input = NewTicket;
    type Owner = ();

    fn create(input: NewTicket, (): ()) -> Result<Self> {
        Ok(Self {
            meta: Meta::new()?,
            project: input.project,
            title: input.title,
            status: "open".to_owned(),
        })
    }
}

impl Update for Ticket {
    type Input = TicketChange;

    fn update(&mut self, change: TicketChange) -> Result<()> {
        self.title = change.title;
        Ok(())
    }
}
