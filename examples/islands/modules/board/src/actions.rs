use sofie::prelude::*;

use crate::note::{Note, factory};

#[derive(Input)]
pub struct WriteNote {
    title: String,
    body: String,
}

pub async fn write_note(input: WriteNote) -> Result<Note> {
    factory::write(input.title, input.body)
}
