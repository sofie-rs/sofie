pub mod factory;

use sofie::prelude::*;

#[derive(Aggregate)]
pub struct Note {
    meta: Meta,
    title: String,
    body: String,
}
