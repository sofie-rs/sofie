use http::HeaderValue;
use sofie_core::{Error, Result};

use crate::{App, Area, Serving};

const BEFORE_NONCE: &str = "default-src 'none'; script-src 'self' 'nonce-";

/// Hosts an area's pages may reach besides their own; nothing until told. Each source is
/// `https://host[:port]`, its leftmost label `*` for any subdomain.
#[derive(Clone, Debug, Default)]
pub struct ContentSecurity {
    sources: Vec<(Directive, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Directive {
    Script,
    Style,
    Image,
    Font,
    Connect,
    Frame,
    FormAction,
}

impl ContentSecurity {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn script(self, source: &str) -> Self {
        self.with(Directive::Script, source)
    }

    #[must_use]
    pub fn style(self, source: &str) -> Self {
        self.with(Directive::Style, source)
    }

    #[must_use]
    pub fn image(self, source: &str) -> Self {
        self.with(Directive::Image, source)
    }

    #[must_use]
    pub fn font(self, source: &str) -> Self {
        self.with(Directive::Font, source)
    }

    #[must_use]
    pub fn connect(self, source: &str) -> Self {
        self.with(Directive::Connect, source)
    }

    /// For frames the page embeds; who may frame the page stays `'none'`.
    #[must_use]
    pub fn frame(self, source: &str) -> Self {
        self.with(Directive::Frame, source)
    }

    #[must_use]
    pub fn form_action(self, source: &str) -> Self {
        self.with(Directive::FormAction, source)
    }

    fn with(mut self, directive: Directive, source: &str) -> Self {
        self.sources.push((directive, source.to_owned()));
        self
    }
}

impl Serving {
    /// Replaces the area's earlier sources; those given to `content_security_all` add to them.
    #[must_use]
    pub fn content_security<A: Area>(mut self, sources: ContentSecurity) -> Self {
        self.content_security.insert(A::NAME, sources);
        self
    }

    #[must_use]
    pub fn content_security_all(mut self, sources: ContentSecurity) -> Self {
        self.content_security_everywhere = sources;
        self
    }
}

/// The policy after the nonce, built once at boot, so a page only joins the two.
pub(crate) struct AreaContentSecurity {
    after_nonce: String,
}

impl Default for AreaContentSecurity {
    fn default() -> Self {
        Self::of(&[])
    }
}

impl AreaContentSecurity {
    fn of(sources: &[(Directive, String)]) -> Self {
        let listed = |directive: Directive| {
            sources
                .iter()
                .filter(move |(named, _)| *named == directive)
                .flat_map(|(_, source)| [" ", source.as_str()])
                .collect::<String>()
        };
        let only_if_listed = |name: &str, directive: Directive| {
            let listed = listed(directive);
            if listed.is_empty() {
                return String::new();
            }
            format!(" {name}{listed};")
        };
        let after_nonce = format!(
            "'{}; style-src 'self'{}; img-src 'self' data:{}; font-src 'self'{}; \
             connect-src 'self'{};{} form-action 'self'{}; frame-ancestors 'none'; \
             base-uri 'none'",
            listed(Directive::Script),
            listed(Directive::Style),
            listed(Directive::Image),
            listed(Directive::Font),
            listed(Directive::Connect),
            only_if_listed("frame-src", Directive::Frame),
            listed(Directive::FormAction),
        );
        Self { after_nonce }
    }

    pub(crate) fn header(&self, nonce: &str) -> Result<HeaderValue> {
        HeaderValue::try_from(format!("{BEFORE_NONCE}{nonce}{}", self.after_nonce))
            .map_err(|error| Error::internal("building the content security policy", error))
    }
}

pub(crate) fn prepare(app: &mut App) -> Result<()> {
    let everywhere = std::mem::take(&mut app.serving.content_security_everywhere);
    let mut by_area = std::mem::take(&mut app.serving.content_security);
    for route in &app.routes {
        by_area.entry(route.audience.name).or_default();
    }
    for (area, own) in by_area {
        let sources = everywhere
            .sources
            .iter()
            .chain(&own.sources)
            .map(|(directive, source)| Ok((*directive, exact_source(source)?)))
            .collect::<Result<Vec<_>>>()
            .map_err(|error| {
                Error::internal(
                    format!("the content security policy of area `{area}`"),
                    error,
                )
            })?;
        app.areas.entry(area).or_default().content_security = AreaContentSecurity::of(&sources);
    }
    Ok(())
}

/// Only HTTPS hosts: a keyword such as `'unsafe-inline'`, a scheme or `*` would undo the policy.
fn exact_source(text: &str) -> Result<String> {
    let invalid = || {
        Error::internal_message(format!(
            "`{text}` is not a source such as https://js.example.com or https://*.example.com"
        ))
    };
    let authority = text.strip_prefix("https://").ok_or_else(invalid)?;
    let host = authority.strip_prefix("*.").unwrap_or(authority);
    let valid = !host.is_empty()
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':'))
        && host.parse::<http::uri::Authority>().is_ok();
    if !valid {
        return Err(invalid());
    }
    Ok(text.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(sources: &[(Directive, &str)]) -> String {
        let sources: Vec<_> = sources
            .iter()
            .map(|(directive, source)| (*directive, (*source).to_owned()))
            .collect();
        let header = AreaContentSecurity::of(&sources).header("abc").unwrap();
        header.to_str().unwrap().to_owned()
    }

    #[test]
    fn the_default_allows_only_the_pages_own_and_nonced_scripts() {
        assert_eq!(
            policy(&[]),
            "default-src 'none'; script-src 'self' 'nonce-abc'; style-src 'self'; \
             img-src 'self' data:; font-src 'self'; connect-src 'self'; \
             form-action 'self'; frame-ancestors 'none'; base-uri 'none'"
        );
    }

    #[test]
    fn sources_join_their_directives_and_frames_appear_only_when_listed() {
        assert_eq!(
            policy(&[
                (Directive::Script, "https://js.provider.example"),
                (Directive::Script, "https://*.js.provider.example"),
                (Directive::Frame, "https://js.provider.example"),
                (Directive::Connect, "https://api.provider.example"),
                (Directive::Font, "https://fonts.example"),
                (Directive::FormAction, "https://pay.provider.example"),
            ]),
            "default-src 'none'; script-src 'self' 'nonce-abc' https://js.provider.example \
             https://*.js.provider.example; style-src 'self'; img-src 'self' data:; \
             font-src 'self' https://fonts.example; connect-src 'self' https://api.provider.example; \
             frame-src https://js.provider.example; form-action 'self' https://pay.provider.example; \
             frame-ancestors 'none'; base-uri 'none'"
        );
    }

    #[test]
    fn a_source_is_an_https_host_or_its_subdomains() {
        assert_eq!(
            exact_source("https://JS.Provider.example").unwrap(),
            "https://js.provider.example"
        );
        assert!(exact_source("https://*.provider.example:8443").is_ok());
        for invalid in [
            "*",
            "https://*",
            "https://",
            "http://js.provider.example",
            "js.provider.example",
            "'unsafe-inline'",
            "'unsafe-eval'",
            "data:",
            "https://js.provider.example/v3",
            "https://a.*.provider.example",
            "https://user@provider.example",
            "https://js.provider.example; script-src *",
            "https://js.provider.example 'unsafe-inline'",
        ] {
            assert!(exact_source(invalid).is_err(), "{invalid}");
        }
    }
}
