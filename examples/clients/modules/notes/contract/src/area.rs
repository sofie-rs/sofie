use sofie::http::{Area, Client, Visitor};

/// The notes screens compiled into Svelte components.
pub struct Board;

impl Area for Board {
    const NAME: &'static str = "board";
    const PREFIX: &'static str = "/board";
    const SIGN_IN: &'static str = "/";
    const CLIENT: Client = Client::Svelte;
    type Principal = Visitor;
}

/// The same screens served as HTML over the wire.
pub struct Desk;

impl Area for Desk {
    const NAME: &'static str = "desk";
    const PREFIX: &'static str = "/desk";
    const SIGN_IN: &'static str = "/";
    type Principal = Visitor;
}
