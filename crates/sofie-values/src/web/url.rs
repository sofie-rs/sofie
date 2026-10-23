use std::fmt;

use http::Uri;
use http::uri::PathAndQuery;
use sofie_core::{Error, Result};

const MAXIMUM_LENGTH: usize = 2048;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Url {
    text: String,
}

impl Url {
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() > MAXIMUM_LENGTH {
            return Err(invalid_url("must have at most 2048 characters"));
        }
        // `Uri` drops a fragment silently, so it is checked apart.
        let (address, fragment) = match text.split_once('#') {
            Some((address, fragment)) => (address, Some(fragment)),
            None => (text, None),
        };
        if fragment.is_some_and(|fragment| !is_fragment(fragment)) {
            return Err(invalid_url(
                "must not have spaces or a second # in its fragment",
            ));
        }
        let uri: Uri = address
            .parse()
            .map_err(|_| invalid_url("must be an address such as https://example.com"))?;
        let scheme = uri
            .scheme_str()
            .map(str::to_ascii_lowercase)
            .filter(|scheme| matches!(scheme.as_str(), "http" | "https"))
            .ok_or_else(|| invalid_url("must start with http:// or https://"))?;
        let authority = uri
            .authority()
            .filter(|authority| !authority.host().is_empty())
            .ok_or_else(|| invalid_url("must name a host"))?
            .as_str();
        if authority.contains('@') {
            return Err(invalid_url("must not carry a user name or password"));
        }
        let path = uri.path_and_query().map_or("/", PathAndQuery::as_str);
        let mut text = format!("{scheme}://{}{path}", authority.to_ascii_lowercase());
        if let Some(fragment) = fragment {
            text.push('#');
            text.push_str(fragment);
        }
        Ok(Self { text })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

fn is_fragment(fragment: &str) -> bool {
    fragment
        .bytes()
        .all(|byte| byte.is_ascii_graphic() && byte != b'#')
}

fn invalid_url(message: &str) -> Error {
    Error::invalid("url", message)
}

impl fmt::Display for Url {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)
    }
}

sofie_core::text_value!(Url, text);

crate::present::present_text!(Url, crate::Input::typed(&[("type", "url")]), Exact);

#[cfg(test)]
mod tests {
    use sofie_core::SqlValue;

    use super::*;
    use crate::testing::{assert_refused, round_trip};

    #[test]
    fn accepts_absolute_web_addresses() {
        for (text, normalized) in [
            ("https://example.com", "https://example.com/"),
            (
                "HTTPS://Example.COM/Shop?Page=2",
                "https://example.com/Shop?Page=2",
            ),
            ("http://localhost:8080/a/b", "http://localhost:8080/a/b"),
            (
                "https://example.com/docs#Install",
                "https://example.com/docs#Install",
            ),
            ("https://[::1]:443/", "https://[::1]:443/"),
        ] {
            assert_eq!(Url::parse(text).unwrap().as_str(), normalized, "{text}");
        }
    }

    #[test]
    fn refuses_other_schemes_relative_addresses_and_credentials() {
        let too_long = format!("https://example.com/{}", "a".repeat(2048));
        assert_refused(
            Url::parse,
            &[
                "",
                "example.com",
                "/relative/path",
                "ftp://example.com/file",
                "javascript:alert(1)",
                "mailto:jane@example.com",
                "https://jane:secret@example.com/",
                "https://example.com/a b",
                "https://example.com/#a#b",
                "https://example.com/#a b",
                "https:///path",
                "https://exämple.com/",
                &too_long,
            ],
        );
    }

    #[test]
    fn round_trips_through_a_text_column_and_json() {
        let url = Url::parse("https://example.com/shop?page=2#top").unwrap();
        assert_eq!(
            round_trip(&url),
            vec![SqlValue::Text("https://example.com/shop?page=2#top".into())]
        );
        let json = serde_json::to_value(&url).unwrap();
        assert_eq!(serde_json::from_value::<Url>(json).unwrap(), url);
        assert_eq!(url.to_string(), url.as_str());
    }
}
