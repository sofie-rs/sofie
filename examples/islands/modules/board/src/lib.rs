pub mod actions;
pub mod module;
mod note;
pub mod queries;
mod wiring;

mod areas {
    pub mod web {
        pub mod routes;
    }
}

pub use note::Note;
