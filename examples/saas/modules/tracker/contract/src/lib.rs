//! What other modules may use of module `tracker`.
pub mod area;
pub mod events;
mod r#gen;
pub mod webhooks;

pub use events::TaskCompleted;
pub use r#gen::{permissions, pruned, routes};
pub use webhooks::TASK_COMPLETED;
