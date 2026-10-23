use std::sync::Arc;

use sofie_core::{Error, Result, SqlValue, Uuid};

use crate::condition::Kind;
use crate::statement::Criteria;
use crate::{BoxFuture, Db, Migrations, Read};

/// Meilisearch's default `maxTotalHits`; a read filtering matches further sees at most these.
pub const MATCHES_READ: usize = 1000;

pub trait SearchEngine: Send + Sync + 'static {
    /// Best first; with an owner, only that owner's.
    fn matching<'a>(
        &'a self,
        index: &'a SearchIndex,
        words: &'a str,
        owner: Option<Uuid>,
    ) -> BoxFuture<'a, Result<Vec<Uuid>>>;

    /// Returns once the engine has taken them, which may be before they are found.
    fn put<'a>(
        &'a self,
        index: &'a SearchIndex,
        documents: &'a [Document],
    ) -> BoxFuture<'a, Result<()>>;

    fn remove<'a>(&'a self, index: &'a SearchIndex, ids: &'a [Uuid]) -> BoxFuture<'a, Result<()>>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchIndex {
    pub(crate) module: &'static str,
    pub(crate) name: &'static str,
    pub(crate) owner: Option<&'static str>,
    pub(crate) hidden: Option<&'static str>,
    pub(crate) fields: &'static [crate::SearchedField],
}

impl SearchIndex {
    /// For an engine's own tests; the database's come from the aggregates' tables.
    #[must_use]
    pub const fn new(
        module: &'static str,
        name: &'static str,
        owner: Option<&'static str>,
    ) -> Self {
        Self {
            module,
            name,
            owner,
            hidden: None,
            fields: &[],
        }
    }

    #[must_use]
    pub fn uid(&self) -> String {
        format!("{}_{}", self.module, self.name)
    }

    #[must_use]
    pub const fn owned(&self) -> bool {
        self.owner.is_some()
    }
}

/// `text` is the record's search text, recomputed from the fields `Searched` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    pub id: Uuid,
    pub text: String,
    pub owner: Option<Uuid>,
}

pub(crate) struct Installed {
    pub(crate) engine: Arc<dyn SearchEngine>,
    pub(crate) indexes: Vec<SearchIndex>,
}

impl Installed {
    pub(crate) fn of(db: &Db) -> Result<&Self> {
        db.search.get().ok_or_else(|| {
            Error::internal_message(
                "no search engine is installed, so the database matches searches itself; set \
                 SOFIE_SEARCH=meilisearch",
            )
        })
    }

    pub(crate) fn find(&self, module: &str, name: &str) -> Option<&SearchIndex> {
        self.indexes
            .iter()
            .find(|index| index.module == module && index.name == name)
    }

    pub(crate) fn index(&self, module: &str, name: &str) -> Result<&SearchIndex> {
        self.find(module, name).ok_or_else(|| {
            Error::internal_message(format!(
                "`{module}.{name}` is searched, but no module's migrations list it among \
                     their aggregates; run `sofie gen`"
            ))
        })
    }
}

/// A database keeps the first engine installed: the worker's may be the app's own.
pub fn install_search(db: &Db, engine: Arc<dyn SearchEngine>, migrations: &[Migrations]) {
    db.search.get_or_init(|| Installed {
        engine,
        indexes: migrations
            .iter()
            .flat_map(|migrations| migrations.aggregates)
            .filter_map(crate::AggregateTable::search_index)
            .collect(),
    });
}

/// The database still filters the engine's ids by the read's other conditions, its owner's too.
pub(crate) async fn matched(mut criteria: Criteria, read: &Read) -> Result<Criteria> {
    let Some(installed) = read.db.search.get() else {
        return Ok(criteria);
    };
    let mut searches = Vec::new();
    for condition in &mut criteria.conditions {
        condition.searches(true, &mut searches);
    }
    for (search, own_records) in searches {
        let Kind::TextSearch {
            words,
            index: (module, name),
            ..
        } = &*search
        else {
            continue;
        };
        let index = installed.index(module, name)?;
        let owner = read.owner().filter(|_| own_records && index.owned());
        // As on Postgres, no words match nothing; an engine would answer every record.
        let ids = if words.trim().is_empty() {
            Vec::new()
        } else {
            installed.engine.matching(index, words, owner).await?
        };
        *search = Kind::In {
            columns: vec!["id".to_owned()],
            rows: ids.into_iter().map(|id| vec![SqlValue::Uuid(id)]).collect(),
        };
    }
    Ok(criteria)
}
