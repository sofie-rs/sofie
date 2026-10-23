use sofie_core::{Error, Result};
use sofie_values::Secret;

use crate::Address;

const TRANSPORT: &str = "SOFIE_MAIL_TRANSPORT";
const FROM: &str = "SOFIE_MAIL_FROM";
const BASE_URL: &str = "SOFIE_MAIL_BASE_URL";
const SMTP_HOST: &str = "SOFIE_MAIL_SMTP_HOST";
const SMTP_PORT: &str = "SOFIE_MAIL_SMTP_PORT";
const SMTP_SECURITY: &str = "SOFIE_MAIL_SMTP_SECURITY";
const SMTP_USERNAME: &str = "SOFIE_MAIL_SMTP_USERNAME";
const SMTP_PASSWORD: &str = "SOFIE_MAIL_SMTP_PASSWORD";
const PREVIEW_DIR: &str = "SOFIE_MAIL_PREVIEW_DIR";
/// Below the shared build directory, so a preview is never committed.
const DEFAULT_PREVIEW_DIR: &str = "target/mail-preview";

pub(crate) struct Settings {
    pub(crate) from: Address,
    pub(crate) base_url: Option<String>,
    pub(crate) transport: Chosen,
}

pub(crate) enum Chosen {
    Smtp(SmtpSettings),
    /// Debug builds only: a logged mail's links would open accounts to whoever reads the log.
    Log {
        preview: Option<std::path::PathBuf>,
    },
}

#[derive(Clone)]
pub struct SmtpSettings {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) security: Security,
    pub(crate) credentials: Option<Credentials>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Security {
    StartTls,
    Tls,
    /// Never across a network: a relay on the same host, or a test's server.
    None,
}

#[derive(Clone)]
pub(crate) struct Credentials {
    pub(crate) username: String,
    pub(crate) password: Secret<String>,
}

#[cfg(feature = "testing")]
impl SmtpSettings {
    #[must_use]
    pub fn new(host: &str, port: u16, security: Security) -> Self {
        Self {
            host: host.to_owned(),
            port,
            security,
            credentials: None,
        }
    }

    #[must_use]
    pub fn with_credentials(self, username: &str, password: &str) -> Self {
        Self {
            credentials: Some(Credentials {
                username: username.to_owned(),
                password: Secret::new(password.to_owned()),
            }),
            ..self
        }
    }
}

impl Settings {
    pub(crate) fn read(text: impl Fn(&str) -> Result<Option<String>>) -> Result<Self> {
        let required = |name: &str| {
            text(name)?.ok_or_else(|| Error::internal_message(format!("{name} is not set")))
        };
        let from = Address::parse(&required(FROM)?)
            .map_err(|error| Error::internal(format!("{FROM} is not a mail address"), error))?;
        let base_url = sofie_core::base_url(BASE_URL, &text)?
            .map(|variable| base_url(variable.name, &variable.value))
            .transpose()?;
        let transport = match required(TRANSPORT)?.trim() {
            "smtp" => Chosen::Smtp(smtp(&text)?),
            "log" if cfg!(debug_assertions) => Chosen::Log {
                preview: match text(PREVIEW_DIR)?.as_deref().map(str::trim) {
                    Some("off") => None,
                    Some(folder) => Some(folder.into()),
                    None => Some(DEFAULT_PREVIEW_DIR.into()),
                },
            },
            "log" => {
                return Err(Error::internal_message(format!(
                    "{TRANSPORT} `log` writes mails, links included, to the log; \
                     it is for debug builds only"
                )));
            }
            other => {
                return Err(Error::internal_message(format!(
                    "{TRANSPORT} `{other}` is neither `smtp` nor `log`"
                )));
            }
        };
        Ok(Self {
            from,
            base_url,
            transport,
        })
    }
}

pub(crate) fn missing_base_url(purpose: &str) -> Error {
    sofie_core::base_url_missing(BASE_URL, purpose)
}

fn base_url(name: &str, url: &str) -> Result<String> {
    let url = url.trim().trim_end_matches('/');
    let host = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    match host {
        Some(host)
            if !host.is_empty()
                && !host.contains(|character: char| {
                    character.is_whitespace()
                        || character.is_control()
                        || matches!(character, '/' | '?' | '#' | '@' | '"' | '<' | '>' | '\\')
                }) =>
        {
            Ok(url.to_owned())
        }
        _ => Err(Error::internal_message(format!(
            "{name} must be an http or https origin such as https://shop.example.com"
        ))),
    }
}

fn smtp(text: impl Fn(&str) -> Result<Option<String>>) -> Result<SmtpSettings> {
    let host = text(SMTP_HOST)?
        .filter(|host| !host.trim().is_empty())
        .ok_or_else(|| Error::internal_message(format!("{SMTP_HOST} is not set")))?;
    let security = match text(SMTP_SECURITY)?.as_deref().map(str::trim) {
        None | Some("starttls") => Security::StartTls,
        Some("tls") => Security::Tls,
        Some("none") => Security::None,
        Some(other) => {
            return Err(Error::internal_message(format!(
                "{SMTP_SECURITY} `{other}` is none of `starttls`, `tls` and `none`"
            )));
        }
    };
    let port = match text(SMTP_PORT)? {
        Some(port) => port.trim().parse().map_err(|_| {
            Error::internal_message(format!("{SMTP_PORT} `{port}` is not a port number"))
        })?,
        None => default_port(security),
    };
    let credentials = match (text(SMTP_USERNAME)?, text(SMTP_PASSWORD)?) {
        (None, None) => None,
        (Some(_), Some(_)) if security == Security::None => {
            return Err(Error::internal_message(format!(
                "{SMTP_USERNAME} and {SMTP_PASSWORD} would cross the network in clear text; \
                 set {SMTP_SECURITY} to `starttls` or `tls`"
            )));
        }
        (Some(username), Some(password)) => Some(Credentials {
            username,
            password: Secret::new(password),
        }),
        _ => {
            return Err(Error::internal_message(format!(
                "{SMTP_USERNAME} and {SMTP_PASSWORD} are set together or not at all"
            )));
        }
    };
    Ok(SmtpSettings {
        host: host.trim().to_owned(),
        port,
        security,
        credentials,
    })
}

fn default_port(security: Security) -> u16 {
    match security {
        Security::StartTls => 587,
        Security::Tls => 465,
        Security::None => 25,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(variables: &[(&str, &str)]) -> Result<Settings> {
        Settings::read(|name| {
            Ok(variables
                .iter()
                .find(|(variable, _)| *variable == name)
                .map(|(_, value)| (*value).to_owned()))
        })
    }

    const FROM_SHOP: (&str, &str) = (FROM, "Shop <shop@example.com>");

    #[test]
    fn smtp_defaults_to_starttls_on_587() {
        let settings = read(&[
            FROM_SHOP,
            (TRANSPORT, "smtp"),
            (SMTP_HOST, "smtp.example.com"),
            (BASE_URL, "https://shop.example.com/"),
        ])
        .unwrap();

        let Chosen::Smtp(smtp) = settings.transport else {
            panic!("smtp was chosen");
        };
        assert_eq!((smtp.host.as_str(), smtp.port), ("smtp.example.com", 587));
        assert_eq!(smtp.security, Security::StartTls);
        assert!(smtp.credentials.is_none());
        assert_eq!(settings.from.name(), Some("Shop"));
        assert_eq!(
            settings.base_url.as_deref(),
            Some("https://shop.example.com")
        );
    }

    #[test]
    fn missing_and_malformed_keys_are_named() {
        let cases: [(&[(&str, &str)], &str); 7] = [
            (&[(TRANSPORT, "smtp")], "SOFIE_MAIL_FROM is not set"),
            (&[FROM_SHOP], "SOFIE_MAIL_TRANSPORT is not set"),
            (&[FROM_SHOP, (TRANSPORT, "pigeon")], "`pigeon` is neither"),
            (
                &[FROM_SHOP, (TRANSPORT, "smtp")],
                "SOFIE_MAIL_SMTP_HOST is not set",
            ),
            (
                &[
                    FROM_SHOP,
                    (TRANSPORT, "log"),
                    (BASE_URL, "shop.example.com"),
                ],
                "SOFIE_MAIL_BASE_URL must be",
            ),
            (
                &[
                    FROM_SHOP,
                    (TRANSPORT, "smtp"),
                    (SMTP_HOST, "h"),
                    (SMTP_USERNAME, "u"),
                ],
                "set together or not at all",
            ),
            (
                &[
                    FROM_SHOP,
                    (TRANSPORT, "smtp"),
                    (SMTP_HOST, "h"),
                    (SMTP_SECURITY, "none"),
                    (SMTP_USERNAME, "u"),
                    (SMTP_PASSWORD, "p"),
                ],
                "in clear text",
            ),
        ];
        for (variables, expected) in cases {
            let error = read(variables).err().expect(expected).to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn the_shared_base_url_serves_unless_mail_has_its_own() {
        let log = [FROM_SHOP, (TRANSPORT, "log")];
        let shared = read(&[log[0], log[1], (sofie_core::BASE_URL, "https://shop.test/")]);
        assert_eq!(
            shared.unwrap().base_url.as_deref(),
            Some("https://shop.test")
        );

        let own = (BASE_URL, "https://mail.shop.test");
        let both = read(&[
            log[0],
            log[1],
            (sofie_core::BASE_URL, "https://shop.test"),
            own,
        ]);
        assert_eq!(
            both.unwrap().base_url.as_deref(),
            Some("https://mail.shop.test")
        );

        let malformed = read(&[log[0], log[1], (sofie_core::BASE_URL, "shop.test")]);
        let error = malformed.err().unwrap().to_string();
        assert!(error.starts_with("SOFIE_BASE_URL must be"), "{error}");
    }

    #[test]
    fn a_base_url_holding_markup_or_spaces_is_refused() {
        for url in [
            "https://shop.test\"x",
            "https://shop .test",
            "https://shop.test<",
            "https://a\\b",
        ] {
            assert!(base_url(BASE_URL, url).is_err(), "{url}");
        }
        assert_eq!(
            base_url(BASE_URL, "https://shop.test/").unwrap(),
            "https://shop.test"
        );
    }

    #[test]
    fn implicit_tls_defaults_to_465_and_a_port_can_be_set() {
        let base = [FROM_SHOP, (TRANSPORT, "smtp"), (SMTP_HOST, "h")];
        let tls = read(&[base[0], base[1], base[2], (SMTP_SECURITY, "tls")]).unwrap();
        let custom = read(&[base[0], base[1], base[2], (SMTP_PORT, "2525")]).unwrap();

        let (Chosen::Smtp(tls), Chosen::Smtp(custom)) = (tls.transport, custom.transport) else {
            panic!("smtp was chosen");
        };
        assert_eq!((tls.security, tls.port), (Security::Tls, 465));
        assert_eq!(custom.port, 2525);
    }
}
