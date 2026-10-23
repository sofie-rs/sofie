pub mod area;
pub mod events;
mod r#gen;

pub use area::Customer;
pub use events::{OrderCancelled, OrderPlaced};
pub use r#gen::{pruned, routes};
