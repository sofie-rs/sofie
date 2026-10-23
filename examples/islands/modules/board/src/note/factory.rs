use sofie::prelude::*;

use super::Note;

pub fn write(title: String, body: String) -> Result<Note> {
    Ok(Note {
        meta: Meta::new()?,
        title,
        body,
    })
}
