//! Only the wiring builds an `Outgoing`, so a request leaves the app through an adapter or not at all.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use sofie_client::Vendor;
use sofie_core::{Error, Result, unpoisoned};
use sofie_db::Db;

use crate::config::environment::Environment;
use crate::config::{ConfigError, Entry, FieldSpec, Group, Kind};

pub use sofie_client::{Adapter, NoSettings, Outgoing, Request, Response, Untrusted, call};

pub struct Choice<G: ?Sized> {
    vendor: &'static str,
    build: fn(&Keys<'_>) -> Result<Arc<G>>,
}

impl<G: ?Sized> Choice<G> {
    #[doc(hidden)]
    #[must_use]
    pub const fn new(vendor: &'static str, build: fn(&Keys<'_>) -> Result<Arc<G>>) -> Self {
        Self { vendor, build }
    }
}

pub struct Keys<'environment> {
    environment: &'environment Environment,
    prefix: String,
}

impl Keys<'_> {
    fn read<T: Group>(&self) -> Result<T> {
        read_keys(self.environment, &self.prefix)
    }
}

pub(crate) fn read_keys<T: Group>(environment: &Environment, prefix: &str) -> Result<T> {
    environment
        .read(prefix)
        .map_err(|error| Error::internal(format!("reading the keys under `{prefix}_`"), error))
}

pub fn adapter<A: Adapter<Settings: Group>>(keys: &Keys<'_>) -> Result<A> {
    let outgoing = sofie_client::for_vendor(A::VENDOR, keys.read()?)?;
    Ok(A::new(outgoing, keys.read()?))
}

type Chosen = HashMap<TypeId, Arc<dyn Any + Send + Sync>>;

/// Each gateway's choice is kept per process, so one table serves every app a process builds.
fn loaded() -> &'static RwLock<Chosen> {
    static LOADED: OnceLock<RwLock<Chosen>> = OnceLock::new();
    LOADED.get_or_init(RwLock::default)
}

/// Chosen again by each app a process builds, the last choice kept. A test process builds no
/// adapter: a test provides the gateway's fake itself.
pub fn load<G: ?Sized + Send + Sync + 'static>(
    module: &str,
    gateway: &str,
    choices: &[Choice<G>],
) -> Result<()> {
    if crate::fakes::provided_by_tests() {
        return Ok(());
    }
    let chosen = choose(&Environment::of_process(), module, gateway, choices)?;
    unpoisoned::write(loaded()).insert(TypeId::of::<Arc<G>>(), Arc::new(chosen));
    Ok(())
}

pub fn provide<G: ?Sized + Send + Sync + 'static>(db: &Db) {
    let loaded = unpoisoned::read(loaded());
    let chosen = loaded
        .get(&TypeId::of::<Arc<G>>())
        .and_then(|chosen| chosen.downcast_ref::<Arc<G>>());
    if let Some(gateway) = chosen {
        sofie_db::provide::<G>(db, Arc::clone(gateway));
    }
}

fn choose<G: ?Sized>(
    environment: &Environment,
    module: &str,
    gateway: &str,
    choices: &[Choice<G>],
) -> Result<Arc<G>> {
    let module = module.to_ascii_uppercase();
    let key = format!("{module}_{}", gateway.to_ascii_uppercase());
    let vendors = || {
        let names: Vec<&str> = choices.iter().map(|choice| choice.vendor).collect();
        names.join(", ")
    };
    let chosen = environment
        .value(&key)
        .map_err(|error| Error::internal(format!("reading `{key}`"), error))?;
    let choice = match (chosen.as_deref(), choices) {
        (Some(vendor), _) => choices
            .iter()
            .find(|choice| choice.vendor == vendor)
            .ok_or_else(|| Error::internal_message(format!("{key} is not one of {}", vendors())))?,
        (None, [only]) => only,
        (None, _) => {
            return Err(Error::internal_message(format!(
                "{key} is not set; it names one of {}",
                vendors()
            )));
        }
    };
    let prefix = format!("{module}_{}", choice.vendor.to_ascii_uppercase());
    (choice.build)(&Keys {
        environment,
        prefix,
    })
}

const fn optional(name: &'static str, key: &'static str, kind: Kind) -> FieldSpec {
    FieldSpec {
        name,
        key,
        optional: true,
        kind,
        fallback: None,
    }
}

/// sofie-client lies below this crate, where `#[derive(sofie::Config)]` cannot reach.
impl Group for Vendor {
    const FIELDS: &'static [FieldSpec] = &[
        optional("base_url", "BASE_URL", Kind::Text),
        optional("hosts", "HOSTS", Kind::Text),
        optional("private", "PRIVATE", Kind::Switch),
        optional("connect_timeout_ms", "CONNECT_TIMEOUT_MS", Kind::Number),
        optional("timeout_ms", "TIMEOUT_MS", Kind::Number),
        optional("max_response_bytes", "MAX_RESPONSE_BYTES", Kind::Number),
        optional("retries", "RETRIES", Kind::Number),
        optional("redirects", "REDIRECTS", Kind::Number),
    ];

    fn build(entry: &Entry<'_>) -> std::result::Result<Self, ConfigError> {
        Ok(Self {
            base_url: entry.read(&Self::FIELDS[0])?,
            hosts: entry.read(&Self::FIELDS[1])?,
            private: entry.read(&Self::FIELDS[2])?,
            connect_timeout_ms: entry.read(&Self::FIELDS[3])?,
            timeout_ms: entry.read(&Self::FIELDS[4])?,
            max_response_bytes: entry.read(&Self::FIELDS[5])?,
            retries: entry.read(&Self::FIELDS[6])?,
            redirects: entry.read(&Self::FIELDS[7])?,
        })
    }
}

impl Group for NoSettings {
    const FIELDS: &'static [FieldSpec] = &[];

    fn build(_entry: &Entry<'_>) -> std::result::Result<Self, ConfigError> {
        Ok(Self {})
    }
}

#[cfg(test)]
mod tests {
    use sofie_client::{NoSettings, Outgoing};

    use super::*;

    trait Mail: Send + Sync {
        fn vendor(&self) -> &'static str;
    }

    struct Smtp;
    struct Postmark {
        token: String,
    }

    #[derive(sofie_macros::Config)]
    struct PostmarkSettings {
        token: String,
    }

    impl Adapter for Smtp {
        const VENDOR: &'static str = "smtp";
        type Settings = NoSettings;

        fn new(_http: Outgoing, _settings: NoSettings) -> Self {
            Self
        }
    }

    impl Adapter for Postmark {
        const VENDOR: &'static str = "postmark";
        type Settings = PostmarkSettings;

        fn new(_http: Outgoing, settings: PostmarkSettings) -> Self {
            Self {
                token: settings.token,
            }
        }
    }

    impl Mail for Smtp {
        fn vendor(&self) -> &'static str {
            Self::VENDOR
        }
    }

    impl Mail for Postmark {
        fn vendor(&self) -> &'static str {
            assert_eq!(self.token, "pm-token");
            Self::VENDOR
        }
    }

    fn choices() -> [Choice<dyn Mail>; 2] {
        [
            Choice::new(Smtp::VENDOR, |keys| Ok(Arc::new(adapter::<Smtp>(keys)?))),
            Choice::new(Postmark::VENDOR, |keys| {
                Ok(Arc::new(adapter::<Postmark>(keys)?))
            }),
        ]
    }

    fn chosen(variables: &[(&str, &str)]) -> Result<Arc<dyn Mail>> {
        choose(&Environment::of(variables), "notify", "mail", &choices())
    }

    #[test]
    fn the_module_s_key_chooses_the_vendor_whose_keys_build_the_adapter() {
        let postmark = chosen(&[
            ("NOTIFY_MAIL", "postmark"),
            ("NOTIFY_POSTMARK_BASE_URL", "https://api.postmarkapp.com"),
            ("NOTIFY_POSTMARK_TOKEN", "pm-token"),
        ])
        .unwrap();
        let smtp = chosen(&[("NOTIFY_MAIL", "smtp")]).unwrap();

        assert_eq!(postmark.vendor(), "postmark");
        assert_eq!(smtp.vendor(), "smtp");
    }

    #[test]
    fn an_unknown_or_missing_choice_and_a_missing_key_are_named() {
        let error = |variables: &[(&str, &str)]| chosen(variables).err().unwrap().to_string();

        assert_eq!(
            error(&[("NOTIFY_MAIL", "sendgrid")]),
            "NOTIFY_MAIL is not one of smtp, postmark"
        );
        assert_eq!(
            error(&[]),
            "NOTIFY_MAIL is not set; it names one of smtp, postmark"
        );
        assert!(
            error(&[("NOTIFY_MAIL", "postmark")]).contains("NOTIFY_POSTMARK_TOKEN is not set"),
            "{}",
            error(&[("NOTIFY_MAIL", "postmark")])
        );
    }
}
