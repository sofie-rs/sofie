use notes_contract::Desk;
use sofie::prelude::*;

use crate::{actions, queries};

pub fn routes(routes: &mut Routes<Desk>) {
    routes.screen("/notes", "notes", queries::list_notes);
    routes
        .post("/notes", actions::write_note)
        .then_screen("notes", "notes");
    routes
        .post("/notes/{note}/pin", actions::pin_note)
        .then_screen("notes", "notes");
}
