use sofie_core::{Error, Result};

#[cfg(feature = "search-meilisearch")]
mod meilisearch;

#[cfg(feature = "search-meilisearch")]
pub(crate) use meilisearch::{Search, configured};

const SEARCH: &str = "SOFIE_SEARCH";

enum Engine {
    Database,
    Meilisearch,
}

fn chosen(lookup: &impl Fn(&str) -> Result<Option<String>>) -> Result<Engine> {
    match lookup(SEARCH)?.as_deref().unwrap_or("database") {
        "database" => Ok(Engine::Database),
        "meilisearch" => Ok(Engine::Meilisearch),
        other => Err(Error::internal_message(format!(
            "{SEARCH} is `{other}`, not one of database, meilisearch"
        ))),
    }
}

/// A search engine the build left out would otherwise be ignored, and the database searched instead.
#[cfg(not(feature = "search-meilisearch"))]
pub(crate) fn refuse_unbuilt(lookup: &impl Fn(&str) -> Result<Option<String>>) -> Result<()> {
    match chosen(lookup)? {
        Engine::Database => Ok(()),
        Engine::Meilisearch => Err(Error::internal_message(format!(
            "{SEARCH} is `meilisearch`, but the app is built without it; \
             enable sofie's `search-meilisearch` feature"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(variables: &[(&str, &str)]) -> Result<Engine> {
        chosen(&|name: &str| Ok(crate::settings::given(variables, name)))
    }

    #[test]
    fn an_unknown_engine_is_refused_naming_the_known_ones() {
        assert!(matches!(read(&[]), Ok(Engine::Database)));
        assert!(matches!(
            read(&[(SEARCH, "meilisearch")]),
            Ok(Engine::Meilisearch)
        ));
        let unknown = read(&[(SEARCH, "elastic")]).err().unwrap().to_string();
        assert_eq!(
            unknown,
            "SOFIE_SEARCH is `elastic`, not one of database, meilisearch"
        );
    }

    #[cfg(not(feature = "search-meilisearch"))]
    #[test]
    fn meilisearch_is_refused_when_the_build_left_it_out() {
        let lookup = |name: &str| Ok(crate::settings::given(&[(SEARCH, "meilisearch")], name));
        let error = refuse_unbuilt(&lookup).err().unwrap().to_string();
        assert!(error.contains("`search-meilisearch` feature"), "{error}");
        assert!(refuse_unbuilt(&|_: &str| Ok(None)).is_ok());
    }
}
