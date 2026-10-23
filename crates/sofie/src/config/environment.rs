use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;

use super::{Group, Kind};

pub(crate) struct Environment {
    variables: BTreeMap<String, OsString>,
}

impl Environment {
    pub(crate) fn of_process() -> Self {
        let variables = std::env::vars_os()
            .filter_map(|(key, value)| Some((key.into_string().ok()?, value)))
            .collect();
        Self { variables }
    }

    pub(crate) fn read<G: Group>(&self, prefix: &str) -> Result<G, ConfigError> {
        G::build(&Entry {
            environment: self,
            key: prefix.to_owned(),
            fallback: None,
        })
    }

    pub(crate) fn value(&self, key: &str) -> Result<Option<String>, ConfigError> {
        self.variables
            .get(key)
            .map(|value| {
                value
                    .clone()
                    .into_string()
                    .map_err(|_| ConfigError::at(key, "is not Unicode"))
            })
            .transpose()
    }

    /// A group is set by its own fields' keys only, not by a sibling key that shares its prefix.
    fn holds(&self, key: &str, kind: Kind) -> bool {
        let Kind::Group(fields) = kind else {
            return self.variables.contains_key(key);
        };
        fields
            .iter()
            .any(|field| self.holds(&format!("{key}_{}", field.key), field.kind))
    }
}

/// A value is never repeated, as it may be a secret.
#[derive(Debug)]
pub struct ConfigError {
    key: String,
    problem: String,
}

impl ConfigError {
    fn at(key: &str, problem: impl Into<String>) -> Self {
        Self {
            key: key.to_owned(),
            problem: problem.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.key, self.problem)
    }
}

impl std::error::Error for ConfigError {}

/// One key, or the prefix of a group's keys.
pub struct Entry<'environment> {
    environment: &'environment Environment,
    key: String,
    fallback: Option<String>,
}

impl Entry<'_> {
    pub fn read<T: super::Field>(&self, field: &super::FieldSpec) -> Result<T, ConfigError> {
        T::read(&self.field(field))
    }

    /// A module shares the app's database until `sofie split` moves it out, so
    /// its own key may only repeat the app's.
    pub(super) fn read_shared<T: super::Field>(
        &self,
        field: &super::FieldSpec,
    ) -> Result<T, ConfigError> {
        let entry = self.field(field);
        if let Some(fallback) = &entry.fallback {
            let own = self.environment.value(&entry.key)?;
            let shared = self.environment.value(fallback)?;
            if let (Some(own), Some(shared)) = (own, shared)
                && own != shared
            {
                return Err(ConfigError::at(
                    &entry.key,
                    format!(
                        "differs from {fallback}; a module's own database applies only \
                         after `sofie split` moves the module into an app of its own"
                    ),
                ));
            }
        }
        T::read(&entry)
    }

    fn field(&self, field: &super::FieldSpec) -> Entry<'_> {
        let fallback = field.fallback.map(str::to_owned).or_else(|| {
            self.fallback
                .as_ref()
                .map(|group| format!("{group}_{}", field.key))
        });
        Entry {
            environment: self.environment,
            key: format!("{}_{}", self.key, field.key),
            fallback,
        }
    }

    pub(super) fn parse<T, E: fmt::Display>(
        &self,
        expected: &str,
        parse: impl FnOnce(&str) -> Result<T, E>,
    ) -> Result<T, ConfigError> {
        parse(&self.text()?)
            .map_err(|error| ConfigError::at(&self.key, format!("is not {expected}: {error:#}")))
    }

    pub(super) fn is_set(&self, kind: Kind) -> bool {
        self.environment.holds(&self.key, kind)
            || self
                .fallback
                .as_ref()
                .is_some_and(|fallback| self.environment.holds(fallback, kind))
    }

    fn text(&self) -> Result<String, ConfigError> {
        if let Some(value) = self.environment.value(&self.key)? {
            return Ok(value);
        }
        let Some(fallback) = &self.fallback else {
            return Err(ConfigError::at(&self.key, "is not set"));
        };
        self.environment.value(fallback)?.ok_or_else(|| {
            ConfigError::at(&self.key, format!("is not set, and neither is {fallback}"))
        })
    }
}

#[cfg(test)]
mod test_support {
    use std::ffi::OsString;

    use super::Environment;

    impl Environment {
        pub(crate) fn of(variables: &[(&str, &str)]) -> Self {
            let variables = variables
                .iter()
                .map(|(key, value)| ((*key).to_owned(), OsString::from(value)))
                .collect();
            Self { variables }
        }
    }
}
