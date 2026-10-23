use sofie::db::Uuid;
use sofie::prelude::*;

#[derive(Event)]
pub struct OrderPlaced {
    pub order: Uuid,
}

#[derive(Event)]
pub struct OrderCancelled {
    pub order: Uuid,
    pub reason: Option<String>,
}
