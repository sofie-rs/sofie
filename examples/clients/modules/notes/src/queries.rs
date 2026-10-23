use sofie::prelude::*;

use crate::note::Note;

#[derive(ReadModel)]
#[read(from = Note)]
pub struct NoteRow {
    id: Id<Note>,
    title: String,
    price: Money,
    pinned: bool,
}

pub async fn list_notes(read: Read) -> Result<Vec<NoteRow>> {
    NoteRow::query().latest().all(&read).await
}
