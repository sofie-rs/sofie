use sofie::prelude::*;

use crate::{actions, queries};

pub fn routes(routes: &mut Routes<Web>) {
    routes.island("/", "note-editor", queries::latest_note);
    routes.screen("/summary", "summary", queries::summary);
    routes.post("/notes", actions::write_note);
}
