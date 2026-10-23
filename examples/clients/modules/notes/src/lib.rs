pub mod actions;
pub mod module;
mod note;
pub mod queries;
mod wiring;

mod areas {
    pub mod board {
        pub mod routes;
    }
    pub mod desk {
        pub mod routes;
    }
}

pub use note::Note;
