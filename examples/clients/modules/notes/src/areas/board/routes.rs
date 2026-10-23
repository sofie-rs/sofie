use notes_contract::Board;
use sofie::prelude::*;

use crate::{actions, queries};

pub fn routes(routes: &mut Routes<Board>) {
    routes.screen("/notes", "notes", queries::list_notes);
    routes
        .post("/notes", actions::write_note)
        .then_screen("notes", "notes");
    routes
        .post("/notes/{note}/pin", actions::pin_note)
        .then_screen("notes", "notes");
}
