use sofie::db::Db;
use sofie::prelude::*;

mod wiring;

pub use wiring::{MODULES, worker};

pub async fn app(db: Db) -> Result<App> {
    wiring::app(db)
}
