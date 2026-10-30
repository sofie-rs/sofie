use std::env::VarError;

use crate::{Error, Result};

pub fn environment_variable(name: &str) -> Result<Option<String>> {
    unicode(name, std::env::var(name))
}

pub const BASE_URL: &str = "SOFIE_BASE_URL";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variable<'name> {
    pub name: &'name str,
    pub value: String,
}

/// An empty value counts as unset, as `.env.example` lists keys empty.
pub fn base_url(
    own: &str,
    lookup: impl Fn(&str) -> Result<Option<String>>,
) -> Result<Option<Variable<'_>>> {
    for name in [own, BASE_URL] {
        if let Some(value) = lookup(name)?.filter(|value| !value.trim().is_empty()) {
            return Ok(Some(Variable { name, value }));
        }
    }
    Ok(None)
}

#[must_use]
pub fn base_url_missing(own: &str, purpose: &str) -> Error {
    Error::internal_message(format!("neither {own} nor {BASE_URL} is set; {purpose}"))
}

fn unicode(name: &str, value: std::result::Result<String, VarError>) -> Result<Option<String>> {
    match value {
        Ok(value) => Ok(Some(value)),
        Err(VarError::NotPresent) => Ok(None),
        Err(VarError::NotUnicode(_)) => {
            Err(Error::internal_message(format!("{name} is not Unicode")))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    #[test]
    fn a_value_that_is_not_unicode_is_refused() {
        let error = unicode("DATABASE_URL", Err(VarError::NotUnicode(OsString::new())))
            .err()
            .unwrap();
        assert_eq!(error.to_string(), "DATABASE_URL is not Unicode");
    }

    fn given(variables: &[(&str, &str)]) -> impl Fn(&str) -> Result<Option<String>> {
        let variables: Vec<(String, String)> = variables
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        move |name| {
            Ok(variables
                .iter()
                .find(|(variable, _)| variable == name)
                .map(|(_, value)| value.clone()))
        }
    }

    #[test]
    fn a_package_s_own_base_url_overrides_the_shared_one() {
        const OWN: &str = "SOFIE_MAIL_BASE_URL";
        let shared = base_url(OWN, given(&[(BASE_URL, "https://a.test")])).unwrap();
        assert_eq!(
            shared,
            Some(Variable {
                name: BASE_URL,
                value: "https://a.test".to_owned()
            })
        );

        let both = [(BASE_URL, "https://a.test"), (OWN, "https://mail.test")];
        let own = base_url(OWN, given(&both)).unwrap().unwrap();
        assert_eq!((own.name, own.value.as_str()), (OWN, "https://mail.test"));

        let empty_own = [(BASE_URL, "https://a.test"), (OWN, " ")];
        let fallen_back = base_url(OWN, given(&empty_own)).unwrap().unwrap();
        assert_eq!(fallen_back.name, BASE_URL);
        assert_eq!(base_url(OWN, given(&[])).unwrap(), None);
        assert_eq!(
            base_url_missing(OWN, "mails link back to the app").to_string(),
            "neither SOFIE_MAIL_BASE_URL nor SOFIE_BASE_URL is set; mails link back to the app"
        );
    }
}
