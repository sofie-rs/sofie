use std::sync::Arc;

use sofie_client::{Meilisearch, Vendor};
use sofie_core::{Error, Result};
use sofie_db::{Db, Migrations, SearchEngine};

use super::{Engine, SEARCH};

const URL: &str = "SOFIE_SEARCH_MEILISEARCH_URL";
const API_KEY: &str = "SOFIE_SEARCH_MEILISEARCH_API_KEY";
/// A Meilisearch inside the network: private addresses and plain HTTP allowed.
const PRIVATE: &str = "SOFIE_SEARCH_MEILISEARCH_PRIVATE";

/// The engine `SOFIE_SEARCH` chose, installed on every database the process opens.
#[derive(Clone)]
pub(crate) struct Search {
    engine: Arc<dyn SearchEngine>,
    migrations: Vec<Migrations>,
}

impl Search {
    pub(crate) fn install(&self, db: &Db) {
        sofie_db::install_search(db, Arc::clone(&self.engine), &self.migrations);
    }
}

/// `None`, the default `database`, leaves matching to the database, as without the feature.
pub(crate) fn configured(
    lookup: &impl Fn(&str) -> Result<Option<String>>,
    migrations: &[Migrations],
) -> Result<Option<Search>> {
    let engine: Arc<dyn SearchEngine> = match super::chosen(lookup)? {
        Engine::Database => return Ok(None),
        Engine::Meilisearch => Arc::new(meilisearch(lookup)?),
    };
    Ok(Some(Search {
        engine,
        migrations: migrations.to_vec(),
    }))
}

fn meilisearch(lookup: &impl Fn(&str) -> Result<Option<String>>) -> Result<Meilisearch> {
    let url = lookup(URL)?.ok_or_else(|| {
        Error::internal_message(format!("{URL} is not set; {SEARCH}=meilisearch needs it"))
    })?;
    let vendor = Vendor {
        base_url: Some(url),
        private: Some(crate::settings::switch(
            PRIVATE,
            lookup(PRIVATE)?.as_deref(),
        )?),
        ..Vendor::default()
    };
    let http = sofie_client::for_vendor("meilisearch", vendor)?;
    Ok(Meilisearch::new(http, lookup(API_KEY)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(variables: &[(&str, &str)]) -> Result<Option<Search>> {
        let lookup = |name: &str| Ok(crate::settings::given(variables, name));
        configured(&lookup, &[])
    }

    #[test]
    fn the_database_searches_unless_meilisearch_is_chosen_with_its_url() {
        assert!(read(&[]).unwrap().is_none());
        assert!(read(&[(SEARCH, "database")]).unwrap().is_none());
        let chosen = read(&[(SEARCH, "meilisearch"), (URL, "https://search.example.com")]);
        assert!(chosen.unwrap().is_some());

        let missing = read(&[(SEARCH, "meilisearch")]).err().unwrap().to_string();
        assert!(missing.contains(URL), "{missing}");
    }
}
