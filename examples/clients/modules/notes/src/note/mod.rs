pub mod factory;

use sofie::prelude::*;

#[derive(Aggregate)]
pub struct Note {
    meta: Meta,
    title: String,
    body: String,
    price: Money,
    pinned: bool,
}

impl Note {
    pub fn pin(&mut self) {
        self.pinned = true;
    }
}
