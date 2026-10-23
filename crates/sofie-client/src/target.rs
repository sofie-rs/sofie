use http::Uri;
use http::uri::PathAndQuery;

/// Host lowercase and without brackets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) secure: bool,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) path: PathAndQuery,
}

impl Target {
    /// Refuses credentials in the URL, which a log or a redirect would carry further.
    pub(crate) fn parse(url: &str) -> Result<Self, String> {
        let uri: Uri = url
            .parse()
            .map_err(|_| "it is not an absolute URL".to_owned())?;
        let secure = match uri.scheme_str() {
            Some("https") => true,
            Some("http") => false,
            Some(_) => return Err("its scheme is neither https nor http".to_owned()),
            None => return Err("it is not an absolute URL".to_owned()),
        };
        let authority = uri
            .authority()
            .ok_or_else(|| "it names no host".to_owned())?;
        if authority.as_str().contains('@') {
            return Err("it carries credentials".to_owned());
        }
        let host = normalized_host(authority.host());
        if host.is_empty() {
            return Err("it names no host".to_owned());
        }
        Ok(Self {
            secure,
            host,
            port: authority.port_u16().unwrap_or(default_port(secure)),
            path: uri
                .path_and_query()
                .cloned()
                .unwrap_or_else(|| PathAndQuery::from_static("/")),
        })
    }

    /// A path built from input, `/records/{id}` with `id = "../internal"`, must not reach
    /// another endpoint of the vendor: dot segments, fragments and the separators servers
    /// also split on (an encoded `/`, `\`, a `;` parameter) are refused.
    pub(crate) fn join(&self, path: &str) -> Result<Self, String> {
        if !path.starts_with('/') || path.starts_with("//") {
            return Err(format!("`{path}` does not start with a single `/`"));
        }
        let route = path.split_once('?').map_or(path, |(route, _)| route);
        if route.split('/').any(is_dot_segment) || path.contains('#') {
            return Err(format!("`{path}` has a `.` or `..` segment or a `#`"));
        }
        if has_hidden_separator(route) {
            return Err(format!("`{path}` has an encoded `/` or a `\\`"));
        }
        let joined = format!("{}{path}", self.path.path().trim_end_matches('/'));
        Ok(Self {
            path: PathAndQuery::try_from(joined)
                .map_err(|_| format!("`{path}` is not a valid path"))?,
            ..self.clone()
        })
    }

    pub(crate) fn redirected(&self, location: &str) -> Result<Self, String> {
        if location.starts_with('/') && !location.starts_with("//") {
            return Ok(Self {
                path: PathAndQuery::try_from(location)
                    .map_err(|_| "the redirect's location is not a valid path".to_owned())?,
                ..self.clone()
            });
        }
        let next = Self::parse(location)?;
        if self.secure && !next.secure {
            return Err("it leaves HTTPS for plain HTTP".to_owned());
        }
        Ok(next)
    }

    pub(crate) fn same_origin(&self, other: &Self) -> bool {
        self.secure == other.secure && self.host == other.host && self.port == other.port
    }

    pub(crate) fn authority(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.port == default_port(self.secure) {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }
}

pub(crate) fn normalized_host(host: &str) -> String {
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase()
}

fn is_dot_segment(segment: &str) -> bool {
    let decoded = segment.to_ascii_lowercase().replace("%2e", ".");
    let decoded = decoded.split(';').next().unwrap_or_default();
    decoded == "." || decoded == ".."
}

fn has_hidden_separator(route: &str) -> bool {
    let lowered = route.to_ascii_lowercase();
    lowered.contains('\\') || lowered.contains("%2f") || lowered.contains("%5c")
}

fn default_port(secure: bool) -> u16 {
    if secure { 443 } else { 80 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_is_taken_apart_and_a_path_joined_below_its_base() {
        let base = Target::parse("https://API.Vendor.test/v1/").unwrap();
        let joined = base.join("/charges?limit=1").unwrap();

        assert_eq!(joined.host, "api.vendor.test");
        assert_eq!(joined.port, 443);
        assert_eq!(joined.path, "/v1/charges?limit=1");
        assert_eq!(joined.authority(), "api.vendor.test");
        let local = Target::parse("http://[::1]:8080").unwrap();
        assert_eq!((local.host.as_str(), local.path.as_str()), ("::1", "/"));
        assert_eq!(
            base.redirected("next").unwrap_err(),
            "it is not an absolute URL"
        );
        assert_eq!(local.authority(), "[::1]:8080");
    }

    #[test]
    fn credentials_other_schemes_and_paths_leaving_the_host_are_refused() {
        let base = Target::parse("https://api.vendor.test").unwrap();

        assert!(Target::parse("https://user:secret@api.vendor.test/").is_err());
        assert!(Target::parse("ftp://api.vendor.test/").is_err());
        assert!(Target::parse("/relative").is_err());
        assert!(base.join("//elsewhere.test/x").is_err());
        assert!(base.join("charges").is_err());
        for leaving in [
            "/customers/../admin",
            "/customers/%2E%2e/admin",
            "/x/./y",
            "/a#b",
            "/records/..%2Finternal",
            "/records/..%2finternal",
            "/records/..\\internal",
            "/records/a%5Cb",
            "/records/..;/admin",
        ] {
            assert!(base.join(leaving).is_err(), "{leaving}");
        }
        assert!(base.join("/files/a..b?next=../x").is_ok());
        assert_eq!(base.redirected("/next").unwrap().host, "api.vendor.test");
        assert_eq!(
            base.redirected("https://other.test/x").unwrap().host,
            "other.test"
        );
        assert!(base.redirected("http://api.vendor.test/x").is_err());
    }
}
