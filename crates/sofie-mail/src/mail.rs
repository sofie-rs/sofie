use std::fmt;

use serde::{Deserialize, Serialize};
use sofie_core::{Error, Result};
use sofie_values::Email;

const MAXIMUM_SUBJECT: usize = 255;
const MAXIMUM_NAME: usize = 255;
const MAXIMUM_LIST: usize = 64;
/// Each would end the address early in an SMTP command or split it in a header.
const REFUSED_IN_ADDRESS: &[char] = &['<', '>', '(', ')', ',', ';', ':', '"', '[', ']', '\\'];

/// Written into headers and SMTP commands as it is, so it holds nothing that could end either.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "AddressParts")]
pub struct Address {
    name: Option<String>,
    email: Email,
}

#[derive(Deserialize)]
struct AddressParts {
    name: Option<String>,
    email: Email,
}

impl TryFrom<AddressParts> for Address {
    type Error = Error;

    fn try_from(parts: AddressParts) -> Result<Self> {
        let address = Self::new(&parts.email)?;
        match parts.name {
            Some(name) => address.named(&name),
            None => Ok(address),
        }
    }
}

impl Address {
    /// One beyond ASCII is sent only through a server offering SMTPUTF8.
    pub fn new(email: &Email) -> Result<Self> {
        if email.as_str().contains(REFUSED_IN_ADDRESS) {
            return Err(Error::invalid(
                "email",
                "must not contain <>(),;:\"[]\\ to receive mail",
            ));
        }
        Ok(Self {
            name: None,
            email: email.clone(),
        })
    }

    pub fn named(self, name: &str) -> Result<Self> {
        if name.chars().count() > MAXIMUM_NAME || name.chars().any(sofie_core::misleads_display) {
            return Err(Error::invalid(
                "name",
                "must be at most 255 characters without control or invisible formatting characters",
            ));
        }
        Ok(Self {
            name: Some(name.to_owned()),
            ..self
        })
    }

    pub(crate) fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        let Some((name, rest)) = text.split_once('<') else {
            return Self::new(&Email::parse(text)?);
        };
        let email = rest
            .strip_suffix('>')
            .ok_or_else(|| Error::invalid("email", "must end with `>` after `<`"))?;
        let address = Self::new(&Email::parse(email)?)?;
        let name = unquoted(name.trim());
        if name.is_empty() {
            return Ok(address);
        }
        address.named(&name)
    }

    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    #[must_use]
    pub fn email(&self) -> &Email {
        &self.email
    }

    pub(crate) fn domain(&self) -> &str {
        self.email
            .as_str()
            .rsplit_once('@')
            .map_or("localhost", |(_, domain)| domain)
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}", self.email)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Body {
    html: String,
    text: String,
}

impl Body {
    pub(crate) fn rendered(html: String) -> Self {
        let text = crate::text::from_html(&html);
        Self { html, text }
    }

    #[must_use]
    pub fn html(&self) -> &str {
        &self.html
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(try_from = "MailParts")]
pub struct Mail {
    from: Option<Address>,
    to: Vec<Address>,
    reply_to: Option<Address>,
    subject: String,
    body: Body,
    list: Option<String>,
    /// Set when it is sent, as each recipient's link is its own.
    #[serde(skip)]
    unsubscribe: Option<String>,
}

#[derive(Deserialize)]
struct MailParts {
    from: Option<Address>,
    to: Vec<Address>,
    reply_to: Option<Address>,
    subject: String,
    body: Body,
    #[serde(default)]
    list: Option<String>,
}

impl TryFrom<MailParts> for Mail {
    type Error = Error;

    fn try_from(parts: MailParts) -> Result<Self> {
        let [first, others @ ..] = parts.to.as_slice() else {
            return Err(Error::invalid("to", "names no recipient"));
        };
        let mail = Self::new(first.clone(), &parts.subject, parts.body)?;
        let mail = others.iter().cloned().fold(mail, Self::also_to);
        let mail = match parts.list {
            Some(list) => mail.bulk(&list)?,
            None => mail,
        };
        Self {
            from: parts.from,
            reply_to: parts.reply_to,
            ..mail
        }
        .checked()
    }
}

/// One outer pair of quotes removed and `\x` read as `x`, as RFC 5322 quotes a name.
fn unquoted(name: &str) -> String {
    let Some(inner) = name
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return name.to_owned();
    };
    let mut unquoted = String::with_capacity(inner.len());
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => unquoted.extend(characters.next()),
            _ => unquoted.push(character),
        }
    }
    unquoted
}

impl Mail {
    /// The subject is one header line: no control or invisible formatting characters, at most 255 characters.
    pub fn new(to: Address, subject: &str, body: Body) -> Result<Self> {
        if subject.trim().is_empty()
            || subject.chars().count() > MAXIMUM_SUBJECT
            || subject.chars().any(sofie_core::misleads_display)
        {
            return Err(Error::invalid(
                "subject",
                "must be 1 to 255 characters without control or invisible formatting characters",
            ));
        }
        Ok(Self {
            from: None,
            to: vec![to],
            reply_to: None,
            subject: subject.to_owned(),
            body,
            list: None,
            unsubscribe: None,
        })
    }

    /// Sent with a one-click unsubscribe link (RFC 8058) to its one recipient, and not at all
    /// to an address that unsubscribed from `list` or complained of bulk mail.
    pub fn bulk(self, list: &str) -> Result<Self> {
        let named = !list.is_empty()
            && list.len() <= MAXIMUM_LIST
            && list
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if !named {
            return Err(Error::invalid(
                "list",
                "is 1 to 64 lowercase letters, digits and `-`",
            ));
        }
        Ok(Self {
            list: Some(list.to_owned()),
            ..self
        })
    }

    pub(crate) fn checked(self) -> Result<Self> {
        if self.list.is_some() && self.to.len() > 1 {
            return Err(Error::invalid(
                "to",
                "names one recipient in a bulk mail, as its unsubscribe link is that recipient's own",
            ));
        }
        Ok(self)
    }

    #[must_use]
    pub fn list(&self) -> Option<&str> {
        self.list.as_deref()
    }

    #[must_use]
    pub fn unsubscribe(&self) -> Option<&str> {
        self.unsubscribe.as_deref()
    }

    pub(crate) fn sent_to(self, to: Vec<Address>, unsubscribe: Option<String>) -> Self {
        Self {
            to,
            unsubscribe,
            ..self
        }
    }

    #[must_use]
    pub fn also_to(mut self, address: Address) -> Self {
        self.to.push(address);
        self
    }

    #[must_use]
    pub fn replying_to(self, address: Address) -> Self {
        Self {
            reply_to: Some(address),
            ..self
        }
    }

    #[must_use]
    pub fn sent_from(self, address: Address) -> Self {
        Self {
            from: Some(address),
            ..self
        }
    }

    #[must_use]
    pub fn from(&self) -> Option<&Address> {
        self.from.as_ref()
    }

    #[must_use]
    pub fn to(&self) -> &[Address] {
        &self.to
    }

    #[must_use]
    pub fn reply_to(&self) -> Option<&Address> {
        self.reply_to.as_ref()
    }

    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    #[must_use]
    pub fn body(&self) -> &Body {
        &self.body
    }

    pub(crate) fn with_default_sender(self, default: &Address) -> Self {
        Self {
            from: Some(self.from.unwrap_or_else(|| default.clone())),
            ..self
        }
    }
}

/// Addresses masked, subject and body left out: either may carry personal data.
impl fmt::Debug for Mail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Mail")
            .field("to", &self.to)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn email(text: &str) -> Email {
        Email::parse(text).unwrap()
    }

    fn body() -> Body {
        Body::rendered("<p>Hello</p>".to_owned())
    }

    #[test]
    fn an_address_that_could_end_a_command_or_header_is_refused() {
        for refused in ["a>b@example.com", "a,b@example.com"] {
            let address = Email::parse(refused).and_then(|email| Address::new(&email));
            assert!(address.is_err(), "{refused}");
        }
        assert!(Address::new(&email("çağrı@example.com")).is_ok());
        let named = Address::new(&email("ada@example.com")).unwrap();
        assert!(named.clone().named("Ada\r\nBcc: x").is_err());
        assert_eq!(
            named.named("Ada Lovelace").unwrap().name(),
            Some("Ada Lovelace")
        );
    }

    #[test]
    fn a_quoted_from_name_is_unescaped_once() {
        let named = Address::parse(r#""Shop \"Main\"" <shop@example.com>"#).unwrap();

        assert_eq!(named.name(), Some("Shop \"Main\""));
    }

    #[test]
    fn a_from_setting_is_read_with_or_without_a_name() {
        let bare = Address::parse("shop@example.com").unwrap();
        let named = Address::parse("\"Shop\" <Shop@Example.com>").unwrap();

        assert_eq!(
            (bare.name(), bare.email().as_str()),
            (None, "shop@example.com")
        );
        assert_eq!(
            (named.name(), named.email().as_str()),
            (Some("Shop"), "shop@example.com")
        );
        assert!(Address::parse("Shop <shop@example.com").is_err());
    }

    #[test]
    fn a_subject_is_one_short_line() {
        let to = Address::new(&email("ada@example.com")).unwrap();

        assert!(Mail::new(to.clone(), "Your receipt", body()).is_ok());
        for refused in ["", "  ", "Line\r\nBcc: x", &"x".repeat(256)] {
            assert!(
                Mail::new(to.clone(), refused, body()).is_err(),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn a_stored_mail_is_checked_again_when_read() {
        let to = Address::new(&email("ada@example.com")).unwrap();
        let mail = Mail::new(to, "Your receipt", body()).unwrap();
        let mut stored = serde_json::to_value(&mail).unwrap();

        let read: Mail = serde_json::from_value(stored.clone()).unwrap();
        assert_eq!(read.subject(), "Your receipt");
        stored["subject"] = "Forged\r\nBcc: x".into();
        assert!(serde_json::from_value::<Mail>(stored.clone()).is_err());
        stored["subject"] = "Fine".into();
        stored["to"] = serde_json::json!([]);
        assert!(serde_json::from_value::<Mail>(stored).is_err());
    }

    #[test]
    fn a_stored_bulk_mail_to_two_recipients_is_refused() {
        let to = Address::new(&email("ada@example.com")).unwrap();
        let mail = Mail::new(to.clone(), "News", body()).unwrap();
        let mut stored = serde_json::to_value(mail.bulk("news").unwrap()).unwrap();

        stored["to"] = serde_json::to_value([&to, &to]).unwrap();

        assert!(serde_json::from_value::<Mail>(stored).is_err());
    }

    #[test]
    fn debug_shows_no_address_subject_or_body() {
        let to = Address::new(&email("ada@example.com")).unwrap();
        let mail = Mail::new(to, "Secret plans", body()).unwrap();

        let shown = format!("{mail:?}");
        assert_eq!(shown, "Mail { to: [a***@example.com], .. }");
    }
}
