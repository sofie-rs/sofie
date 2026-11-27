pub mod config;
pub mod module;
mod wiring;

mod areas {
    pub mod web {
        pub mod routes;
    }
    pub mod staff {
        pub mod routes;
    }
    pub mod member {
        pub mod routes;
    }
}
pub mod queries;
