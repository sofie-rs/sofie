//! The exporter's settings, from the environment variables the OpenTelemetry specification names.

use std::env::{self, VarError};
use std::time::Duration;

use http::header::{HeaderName, HeaderValue};
use http::uri::{Scheme, Uri};

use super::common;
use crate::InitError;

const ENDPOINT: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
const HEADERS: &str = "OTEL_EXPORTER_OTLP_HEADERS";
const TIMEOUT: &str = "OTEL_EXPORTER_OTLP_TIMEOUT";
const SERVICE_NAME: &str = "OTEL_SERVICE_NAME";
const RESOURCE_ATTRIBUTES: &str = "OTEL_RESOURCE_ATTRIBUTES";
const SERVICE_NAME_KEY: &str = "service.name";
const FALLBACK_SERVICE_NAME: &str = "sofie-app";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub(crate) struct Settings {
    pub(crate) endpoint: Endpoint,
    pub(crate) headers: Vec<(HeaderName, HeaderValue)>,
    pub(crate) timeout: Duration,
    pub(crate) resource: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(crate) struct Endpoint {
    pub(crate) secure: bool,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) authority: String,
    /// Without a trailing `/`; each signal appends its own path.
    pub(crate) path: String,
}

impl Settings {
    /// None when no endpoint is set: nothing is exported. Exporting needs a Tokio runtime.
    pub(crate) fn from_environment() -> Result<Option<Self>, InitError> {
        let Some(endpoint) = variable(ENDPOINT)? else {
            return Ok(None);
        };
        tokio::runtime::Handle::try_current().map_err(|error| {
            InitError::new(format!("the OTLP exporter needs a Tokio runtime: {error}"))
        })?;
        Ok(Some(Self {
            endpoint: Endpoint::parse(&endpoint)?,
            headers: headers(variable(HEADERS)?.as_deref().unwrap_or_default())?,
            timeout: timeout(variable(TIMEOUT)?.as_deref())?,
            resource: common::resource_body(&resource_attributes(
                variable(SERVICE_NAME)?,
                variable(RESOURCE_ATTRIBUTES)?
                    .as_deref()
                    .unwrap_or_default(),
            )),
        }))
    }
}

impl Endpoint {
    pub(crate) fn parse(text: &str) -> Result<Self, InitError> {
        if text.contains('@') {
            return Err(InitError::new(format!(
                "{ENDPOINT} carries credentials; send them in {HEADERS}"
            )));
        }
        if text.contains(['?', '#']) {
            return Err(InitError::new(format!(
                "{ENDPOINT} has a query or fragment, which is not sent; send credentials in {HEADERS}"
            )));
        }
        let invalid = |reason: &str| InitError::new(format!("{ENDPOINT} `{text}` {reason}"));
        let uri: Uri = text
            .parse()
            .map_err(|error| invalid(&format!("is not a url: {error}")))?;
        let secure = match uri.scheme() {
            Some(scheme) if *scheme == Scheme::HTTPS => true,
            Some(scheme) if *scheme == Scheme::HTTP => false,
            _ => return Err(invalid("must start with http:// or https://")),
        };
        let authority = uri.authority().ok_or_else(|| invalid("names no host"))?;
        let port = authority
            .port_u16()
            .unwrap_or(if secure { 443 } else { 80 });
        Ok(Self {
            secure,
            host: authority
                .host()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_owned(),
            port,
            authority: authority.as_str().to_owned(),
            path: uri.path().trim_end_matches('/').to_owned(),
        })
    }
}

/// Unset and empty mean the same: an empty variable is how a deployment unsets one.
fn variable(name: &str) -> Result<Option<String>, InitError> {
    match env::var(name) {
        Ok(value) if value.trim().is_empty() => Ok(None),
        Ok(value) => Ok(Some(value.trim().to_owned())),
        Err(VarError::NotPresent) => Ok(None),
        Err(error) => Err(InitError::new(format!("reading {name}: {error}"))),
    }
}

/// `key=value,key=value`, each value percent-decoded.
fn headers(text: &str) -> Result<Vec<(HeaderName, HeaderValue)>, InitError> {
    text.split_terminator(',')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').ok_or_else(|| {
                InitError::new(format!("{HEADERS} holds an entry that is not `name=value`"))
            })?;
            let invalid = || {
                InitError::new(format!(
                    "{HEADERS} holds an invalid header `{}`",
                    name.trim()
                ))
            };
            let name = HeaderName::try_from(name.trim()).map_err(|_| invalid())?;
            let value = percent_decoded(value.trim()).ok_or_else(invalid)?;
            let value = HeaderValue::try_from(value).map_err(|_| invalid())?;
            Ok((name, value))
        })
        .collect()
}

fn percent_decoded(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = text.get(index + 1..index + 3)?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// In milliseconds.
fn timeout(text: Option<&str>) -> Result<Duration, InitError> {
    let Some(text) = text else {
        return Ok(DEFAULT_TIMEOUT);
    };
    text.parse().map(Duration::from_millis).map_err(|_| {
        InitError::new(format!(
            "{TIMEOUT} `{text}` is not a number of milliseconds"
        ))
    })
}

/// `OTEL_SERVICE_NAME` wins over a `service.name` in `OTEL_RESOURCE_ATTRIBUTES`; without either,
/// the service is named after the executable.
fn resource_attributes(service_name: Option<String>, listed: &str) -> Vec<(String, String)> {
    let mut attributes: Vec<(String, String)> = Vec::new();
    for (key, value) in listed
        .split_terminator(',')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (key.trim(), value.trim()))
        .filter(|(key, _)| !key.is_empty())
    {
        attributes.retain(|(existing, _)| existing != key);
        attributes.push((key.to_owned(), value.to_owned()));
    }
    let listed_name = attributes
        .iter()
        .position(|(key, _)| key == SERVICE_NAME_KEY)
        .map(|index| attributes.remove(index).1);
    let service_name = service_name.or(listed_name).unwrap_or_else(executable_name);
    let mut all = vec![
        (SERVICE_NAME_KEY.to_owned(), service_name),
        ("telemetry.sdk.language".to_owned(), "rust".to_owned()),
        ("telemetry.sdk.name".to_owned(), "sofie".to_owned()),
        (
            "telemetry.sdk.version".to_owned(),
            env!("CARGO_PKG_VERSION").to_owned(),
        ),
    ];
    all.append(&mut attributes);
    all
}

fn executable_name() -> String {
    env::current_exe()
        .ok()
        .and_then(|path| Some(path.file_stem()?.to_string_lossy().into_owned()))
        .unwrap_or_else(|| FALLBACK_SERVICE_NAME.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_endpoint_keeps_its_path_and_defaults_its_port() {
        let endpoint = Endpoint::parse("https://collector.example/otlp/").unwrap();
        assert!(endpoint.secure);
        assert_eq!(endpoint.host, "collector.example");
        assert_eq!(endpoint.port, 443);
        assert_eq!(endpoint.path, "/otlp");

        let endpoint = Endpoint::parse("http://[::1]:4318").unwrap();
        assert_eq!(endpoint.host, "::1");
        assert_eq!(endpoint.authority, "[::1]:4318");
        assert_eq!(endpoint.path, "");

        assert!(Endpoint::parse("grpc://collector:4317").is_err());
        let error = Endpoint::parse("https://user:hunter2@collector:4318").unwrap_err();
        assert!(!error.to_string().contains("hunter2"), "{error}");
    }

    #[test]
    fn an_endpoint_with_a_query_is_refused_without_quoting_it() {
        let error = Endpoint::parse("https://collector:4318/otlp?api-key=hunter2").unwrap_err();
        assert!(!error.to_string().contains("hunter2"), "{error}");
        assert!(Endpoint::parse("https://collector:4318/otlp#x").is_err());
    }

    #[test]
    fn headers_are_pairs_with_percent_decoded_values() {
        let parsed = headers("api-key=a%20b, x-tenant = shop ,").unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].0, "api-key");
        assert_eq!(parsed[0].1, "a b");
        assert_eq!(parsed[1].1, "shop");

        assert!(headers("no-value").is_err());
    }

    #[test]
    fn an_invalid_header_is_named_without_its_value() {
        for text in ["api-key=sk_live_1%zz", "sk_live_1"] {
            let error = headers(text).unwrap_err().to_string();

            assert!(!error.contains("sk_live_1"), "{error}");
        }
        assert!(
            headers("api-key=x%zz")
                .unwrap_err()
                .to_string()
                .contains("`api-key`")
        );
    }

    #[test]
    fn the_service_name_variable_wins_over_the_listed_one() {
        let named = |service_name: Option<&str>, listed: &str| {
            resource_attributes(service_name.map(str::to_owned), listed)
        };

        let attributes = named(Some("shop"), "service.name=other,region=eu");
        assert_eq!(
            attributes[0],
            ("service.name".to_owned(), "shop".to_owned())
        );
        assert!(attributes.contains(&("region".to_owned(), "eu".to_owned())));
        assert_eq!(
            attributes
                .iter()
                .filter(|(key, _)| key == "service.name")
                .count(),
            1
        );

        assert_eq!(named(None, "service.name=other")[0].1, "other");
    }
}
