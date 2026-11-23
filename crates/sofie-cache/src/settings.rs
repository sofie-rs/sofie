use sofie_core::{Error, Result, environment_variable};

const STORE: &str = "SOFIE_CACHE_STORE";

/// Where entries live: in each instance's memory, or shared, with memory in front when
/// sofie-cache's `memory` feature is on.
#[derive(Debug, PartialEq, Eq)]
pub enum Store {
    #[cfg(feature = "memory")]
    Memory,
    #[cfg(feature = "database")]
    Database,
    /// Keys are named by crate and key, not by app: apps sharing one Redis each need a database
    /// of their own (`redis://host/2`).
    #[cfg(feature = "redis")]
    Redis,
}

const BUILT: &[&str] = &[
    #[cfg(feature = "memory")]
    "memory",
    #[cfg(feature = "database")]
    "database",
    #[cfg(feature = "redis")]
    "redis",
];

impl Store {
    pub fn from_environment() -> Result<Self> {
        Self::read(environment_variable)
    }

    fn read(lookup: impl Fn(&str) -> Result<Option<String>>) -> Result<Self> {
        let chosen = lookup(STORE)?;
        match chosen.as_deref().map(str::trim) {
            #[cfg(feature = "memory")]
            None | Some("memory") => Ok(Self::Memory),
            #[cfg(feature = "database")]
            Some("database") => Ok(Self::Database),
            #[cfg(feature = "redis")]
            Some("redis") => Ok(Self::Redis),
            other => Err(Error::internal_message(format!(
                "{STORE} `{}` names no store this app was built with; it has: {}",
                other.unwrap_or_default(),
                BUILT.join(", ")
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(variables: &[(&str, &str)]) -> Result<Store> {
        Store::read(|name| {
            Ok(variables
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned()))
        })
    }

    #[test]
    fn memory_is_the_store_when_none_is_named() {
        assert_eq!(read(&[]).unwrap(), Store::Memory);
        assert_eq!(read(&[(STORE, "database")]).unwrap(), Store::Database);
    }

    #[test]
    fn an_unknown_store_lists_the_built_ones() {
        let error = read(&[(STORE, "memcached")]).unwrap_err().to_string();

        assert!(error.contains("memory, database, redis"), "{error}");
    }
}
