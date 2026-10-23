use serde::{Deserialize, Serialize};
use sofie_core::{Error, Result, Uuid};
use sofie_values::Email;

const MAXIMUM_TITLE: usize = 200;
const MAXIMUM_BODY: usize = 2000;
/// With the title, small enough that a push always fits one encrypted record.
const MAXIMUM_LINK: usize = 1000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) link: Option<String>,
}

impl Notice {
    /// One line, as a push or a mail subject shows it.
    pub fn new(title: &str) -> Result<Self> {
        let refused = title.trim().is_empty()
            || title.chars().count() > MAXIMUM_TITLE
            || title
                .chars()
                .any(|character| character.is_control() || sofie_core::misleads_display(character));
        if refused {
            return Err(Error::invalid(
                "title",
                "must be one line of 1 to 200 characters, not only spaces, without control or invisible characters",
            ));
        }
        Ok(Self {
            title: title.to_owned(),
            body: String::new(),
            link: None,
        })
    }

    pub fn body(self, body: &str) -> Result<Self> {
        if body.chars().count() > MAXIMUM_BODY {
            return Err(Error::invalid("body", "must be at most 2000 characters"));
        }
        Ok(Self {
            body: body.to_owned(),
            ..self
        })
    }

    /// A path of this app, opened from the inbox, a mail or a push.
    pub fn link(self, path: &str) -> Result<Self> {
        let local = path.starts_with('/')
            && !path.starts_with("//")
            && path.len() <= MAXIMUM_LINK
            && !path.contains(|character: char| character == '\\' || character.is_control());
        if !local {
            return Err(Error::invalid(
                "link",
                "must be a path of this app of at most 1000 bytes, such as /orders/7",
            ));
        }
        Ok(Self {
            link: Some(path.to_owned()),
            ..self
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recipient {
    pub(crate) principal: Uuid,
    pub(crate) email: Option<Email>,
}

impl Recipient {
    #[must_use]
    pub fn new(principal: Uuid) -> Self {
        Self {
            principal,
            email: None,
        }
    }

    /// Without one, the mail channel is skipped.
    #[must_use]
    pub fn email(self, email: &Email) -> Self {
        Self {
            email: Some(email.clone()),
            ..self
        }
    }
}

impl<T> From<sofie_db::Id<T>> for Recipient {
    fn from(id: sofie_db::Id<T>) -> Self {
        Self::new(id.uuid())
    }
}

#[cfg(test)]
mod tests {
    use super::Notice;

    /// A mail's subject refuses a blank one, which failed the whole delivery, inbox included.
    #[test]
    fn a_title_of_only_spaces_is_refused() {
        assert!(Notice::new("   ").is_err());
        assert!(Notice::new(" Shipped ").is_ok());
    }

    #[test]
    fn a_link_is_a_short_path_without_control_characters() {
        let notice = || Notice::new("Shipped").unwrap();
        assert!(notice().link("/orders/7").is_ok());
        assert!(notice().link(&format!("/{}", "a".repeat(999))).is_ok());
        assert!(notice().link(&format!("/{}", "a".repeat(1000))).is_err());
        assert!(notice().link("/orders/\t7").is_err());
    }
}
