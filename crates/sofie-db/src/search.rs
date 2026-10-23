#[cfg(feature = "search-engine")]
mod engine;
#[cfg(feature = "search-engine")]
mod indexing;

use sofie_core::{Cast, ColumnSpec, SqlValue};
use sofie_values::Search;

use crate::condition::Kind;
use crate::{Aggregate, Behavior, Condition, Field, Uses, Write};

#[cfg(feature = "search-engine")]
pub use engine::{Document, MATCHES_READ, SearchEngine, SearchIndex, install_search};
#[cfg(feature = "search-engine")]
pub(crate) use engine::{Installed, matched};
#[cfg(feature = "search-engine")]
pub use indexing::{REINDEX_TASK, Reindexing, SearchIndexes, index_searches, reindex};
#[cfg(all(feature = "search-engine", feature = "retention"))]
pub(crate) use indexing::{index_of, queue_forget};

pub const SEARCH_COLUMN: &str = "search_text";

pub type SearchedField = (&'static str, &'static [ColumnSpec], Search);

pub struct Searched {
    pub fields: &'static [SearchedField],
}

impl Behavior for Searched {
    fn columns(&self) -> &'static [Field] {
        const { &[Field::new(SEARCH_COLUMN, String::COLUMNS, false)] }
    }

    fn written(&self, write: &Write<'_>, values: &mut [SqlValue]) {
        values[0] = SqlValue::Text(self.text(write));
    }

    fn searched(&self) -> bool {
        true
    }

    #[cfg(feature = "search-engine")]
    fn search_fields(&self) -> &'static [SearchedField] {
        self.fields
    }

    #[cfg(feature = "search-engine")]
    fn planned(&self, write: &Write<'_>, plan: &mut crate::Plan<'_>) -> sofie_core::Result<()> {
        if plan.indexes_searches() && indexing::changes_document(write) {
            return indexing::queue(write, plan);
        }
        Ok(())
    }
}

impl Searched {
    fn text(&self, write: &Write<'_>) -> String {
        search_text(self.fields, |column| write.value(column))
    }
}

/// Numbers and times are ranges a filter asks for, not words.
fn worded(fields: &[SearchedField]) -> impl Iterator<Item = (String, &ColumnSpec)> + '_ {
    fields
        .iter()
        .filter(|(_, _, search)| matches!(search, Search::Text | Search::Exact))
        .flat_map(|(name, columns, _)| {
            columns
                .iter()
                .map(|column| (crate::sql::column_name(name, column), column))
        })
}

#[cfg(feature = "search-engine")]
pub(crate) fn text_columns(fields: &[SearchedField]) -> Vec<(String, sofie_core::SqlType)> {
    worded(fields)
        .map(|(column, spec)| (column, spec.ty))
        .collect()
}

pub(crate) fn search_text<'a>(
    fields: &[SearchedField],
    value: impl Fn(&str) -> Option<&'a SqlValue>,
) -> String {
    let mut text = String::new();
    for (column, _) in worded(fields) {
        let word = match value(&column) {
            Some(SqlValue::Text(value)) => value.clone(),
            Some(SqlValue::Uuid(value)) => value.to_string(),
            _ => continue,
        };
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&word);
    }
    text
}

pub trait Searches: Aggregate {
    /// `"quoted words"` match as a phrase and `-word` excludes, where the database parses queries.
    #[must_use]
    fn matching(words: &str) -> Condition<Self> {
        Condition::new(Kind::TextSearch {
            column: SEARCH_COLUMN.to_owned(),
            words: words.to_owned(),
            #[cfg(feature = "search-engine")]
            index: (Self::MODULE, Self::NAME),
        })
    }
}

impl<A: Uses<Searched>> Searches for A {}
