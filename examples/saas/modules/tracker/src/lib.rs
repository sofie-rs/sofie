pub mod actions;
pub mod config;
pub mod listeners;
pub mod module;
pub mod plans;
pub mod queries;
mod task;
mod wiring;

mod areas {
    pub mod staff {
        pub mod routes;
    }
    pub mod team {
        pub mod routes;
    }
    pub mod web {
        pub mod routes;
    }
}

pub use task::{Task, TaskStatus};
