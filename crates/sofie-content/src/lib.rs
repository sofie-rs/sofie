//! Pages written in Markdown: served at `/pages/{slug}` once published, each
//! with its description, canonical URL and link-preview tags, and listed in
//! `/sitemap.xml`. `SOFIE_CONTENT_BASE_URL` is where the app is reached.

#[cfg(feature = "admin")]
mod admin;
mod page;
mod public;

use sofie::Module;
use sofie::db::{AggregateTable, Migrations};

#[cfg(feature = "admin")]
pub use admin::{ROUTES as ADMIN_ROUTES, admin_routes};
#[cfg(feature = "admin")]
pub use page::PageInput;
pub use page::{Markdown, Page};
pub use public::{ROUTES, Settings, install, routes};

pub const MODULE: Module = Module {
    migrations: Some(migrations()),
    screens: &[(
        "sofie_content/web/page",
        sofie::db::embed!(
            "src/screens/page.html",
            compiled = "screens/sofie_content/web/page.sft"
        ),
    )],
    areas: &[|app, module| app.area::<sofie::http::Web>(module, routes)],
    ..Module::new(NAME)
};

const NAME: &str = "sofie_content";

#[must_use]
pub const fn migrations() -> Migrations {
    Migrations::new(
        NAME,
        sofie::db::embed!(
            "migrations",
            ["0000_module_role.sql", "0001_create_page.sql"]
        ),
    )
    .sqlite(sofie::db::embed!(
        "migrations/sqlite",
        ["0000_module_role.sql", "0001_create_page.sql"]
    ))
    .aggregates(AGGREGATES)
}

const AGGREGATES: &[AggregateTable] = &[AggregateTable::of::<Page>()];
