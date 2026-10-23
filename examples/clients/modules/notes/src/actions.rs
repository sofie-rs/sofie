use sofie::prelude::*;

use crate::note::{Note, factory};

#[derive(Input)]
pub struct WriteNote {
    title: String,
    body: String,
    price: Money,
}

pub async fn write_note(input: WriteNote) -> Result<Note> {
    factory::write(input.title, input.body, input.price)
}

pub async fn pin_note(mut note: Note) -> Result<Note> {
    note.pin();
    Ok(note)
}
