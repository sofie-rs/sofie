pub(crate) mod environment;
mod field;

use std::any::type_name;
use std::ops::Deref;
use std::sync::OnceLock;

use sofie_core::{Error, Result};
use sofie_values::Secret;

use environment::Environment;
pub use environment::{ConfigError, Entry};
pub use field::{Field, FieldSpec, Group, Kind};
use sofie_db::Resolve;

pub struct Config<T: 'static>(&'static T);

impl<T> Deref for Config<T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.0
    }
}

/// The messages queue lives in a database too, so both fall back to the app's `DATABASE_*` keys.
macro_rules! database_groups {
    ($($group:ident),*) => {$(
        #[derive(Clone, Debug)]
        pub struct $group {
            url: Secret<String>,
        }

        impl $group {
            #[must_use]
            pub fn url(&self) -> &Secret<String> {
                &self.url
            }
        }

        impl Group for $group {
            const FIELDS: &'static [FieldSpec] = &[FieldSpec {
                name: "url",
                key: "URL",
                optional: false,
                kind: <Secret<String> as Field>::KIND,
                fallback: None,
            }];
            const FALLBACK: Option<&'static str> = Some("DATABASE");

            fn build(entry: &Entry<'_>) -> std::result::Result<Self, ConfigError> {
                Ok(Self {
                    url: entry.read_shared(&Self::FIELDS[0])?,
                })
            }
        }

        impl Loaded for $group {
            fn loaded() -> &'static OnceLock<Self> {
                static LOADED: OnceLock<$group> = OnceLock::new();
                &LOADED
            }
        }
    )*};
}

database_groups!(Database, Queue);

/// One slot per config type, filled at boot, so a handler reads its config without a lock or a lookup.
pub trait Loaded: Sized + 'static {
    fn loaded() -> &'static OnceLock<Self>;
}

/// Configs are fixed for the process's life, so the first app a process builds loads them for every later one, tests included.
pub fn load<T: Group + Loaded + Send + Sync>(module: &str) -> Result<()> {
    if T::loaded().get().is_some() {
        return Ok(());
    }
    let config: T = Environment::of_process()
        .read(&module.to_ascii_uppercase())
        .map_err(|error| Error::internal(format!("reading module `{module}`'s config"), error))?;
    // An app built on another thread may have loaded the same keys meanwhile.
    let _ = T::loaded().set(config);
    Ok(())
}

impl<C: Send, T: Loaded + Send + Sync> Resolve<C> for Config<T> {
    fn resolve(_context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(loaded_config())
    }
}

fn loaded_config<T: Loaded + Sync>() -> Result<Config<T>> {
    let config = T::loaded().get().ok_or_else(|| {
        Error::internal_message(format!(
            "{} was not loaded at boot; run `sofie gen` so the wiring loads it",
            type_name::<T>()
        ))
    })?;
    Ok(Config(config))
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use sofie_values::Currency;

    use super::*;

    #[derive(Debug, sofie_macros::Config)]
    struct OrderConfig {
        invoice_prefix: String,
        payment_window_hours: Option<u64>,
        api_key: Secret<String>,
        database: Database,
        queue: Queue,
        payment: Payment,
        refund: Option<Payment>,
    }

    #[derive(Debug, sofie_macros::Config)]
    struct Payment {
        currency: Currency,
        listen: Option<SocketAddr>,
        live: bool,
    }

    const REQUIRED: [(&str, &str); 5] = [
        ("ORDER_INVOICE_PREFIX", "INV-"),
        ("ORDER_API_KEY", "sk_123"),
        ("DATABASE_URL", "postgres://app"),
        ("ORDER_PAYMENT_CURRENCY", "EUR"),
        ("ORDER_PAYMENT_LIVE", "false"),
    ];

    fn read(variables: &[(&str, &str)]) -> std::result::Result<OrderConfig, String> {
        Environment::of(variables)
            .read("ORDER")
            .map_err(|error| error.to_string())
    }

    fn with(extra: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
        let mut variables = REQUIRED.to_vec();
        variables.extend_from_slice(extra);
        variables
    }

    #[test]
    fn fields_are_read_from_the_module_s_prefixed_keys() {
        let config = read(&with(&[
            ("ORDER_PAYMENT_WINDOW_HOURS", "48"),
            ("ORDER_PAYMENT_LISTEN", "127.0.0.1:8080"),
        ]))
        .unwrap();

        assert_eq!(config.invoice_prefix, "INV-");
        assert_eq!(config.payment_window_hours, Some(48));
        assert_eq!(config.api_key.expose(), "sk_123");
        assert_eq!(config.payment.currency.as_str(), "EUR");
        assert_eq!(config.payment.listen.unwrap().port(), 8080);
        assert!(!config.payment.live);
        assert_eq!(read(&REQUIRED).unwrap().payment_window_hours, None);
    }

    #[test]
    fn a_switch_is_on_or_off_as_well_as_true_or_false() {
        for (text, live) in [
            ("on", true),
            ("true", true),
            ("off", false),
            ("false", false),
        ] {
            let config = read(&with(&[("ORDER_PAYMENT_LIVE", text)])).unwrap();
            assert_eq!(config.payment.live, live, "{text}");
        }
    }

    #[test]
    fn an_optional_group_is_read_once_any_of_its_keys_is_set() {
        assert!(read(&REQUIRED).unwrap().refund.is_none());

        let error = read(&with(&[("ORDER_REFUND_LIVE", "true")])).unwrap_err();
        assert_eq!(error, "ORDER_REFUND_CURRENCY is not set");

        let refund = read(&with(&[
            ("ORDER_REFUND_LIVE", "true"),
            ("ORDER_REFUND_CURRENCY", "USD"),
        ]))
        .unwrap()
        .refund
        .unwrap();
        assert!(refund.live);
    }

    #[test]
    fn database_and_queue_fall_back_to_the_app_s_database() {
        let shared = read(&REQUIRED).unwrap();
        assert_eq!(shared.database.url().expose(), "postgres://app");
        assert_eq!(shared.queue.url().expose(), "postgres://app");

        let repeated = read(&with(&[
            ("ORDER_DATABASE_URL", "postgres://app"),
            ("ORDER_QUEUE_URL", "postgres://app"),
        ]));
        assert_eq!(repeated.unwrap().queue.url().expose(), "postgres://app");
    }

    #[test]
    fn a_module_database_or_queue_apart_from_the_app_s_is_refused_until_split() {
        for key in ["ORDER_DATABASE_URL", "ORDER_QUEUE_URL"] {
            let error = read(&with(&[(key, "postgres://orders")])).unwrap_err();
            assert!(
                error.starts_with(&format!("{key} differs from DATABASE_URL;"))
                    && error.contains("`sofie split`")
                    && !error.contains("postgres://"),
                "{error}"
            );
        }
    }

    #[test]
    fn a_missing_key_is_named() {
        let error = read(&REQUIRED[1..]).unwrap_err();
        assert_eq!(error, "ORDER_INVOICE_PREFIX is not set");

        let without_database: Vec<_> = REQUIRED
            .into_iter()
            .filter(|(key, _)| *key != "DATABASE_URL")
            .collect();
        assert_eq!(
            read(&without_database).unwrap_err(),
            "ORDER_DATABASE_URL is not set, and neither is DATABASE_URL"
        );
    }

    #[test]
    fn a_malformed_key_is_named_without_its_value() {
        let malformed = |key: &'static str, value: &'static str| {
            let mut variables: Vec<_> = REQUIRED
                .into_iter()
                .filter(|(set, _)| *set != key)
                .collect();
            variables.push((key, value));
            let error = read(&variables).unwrap_err();
            assert!(!error.contains(value), "{error}");
            error
        };

        assert!(
            malformed("ORDER_PAYMENT_WINDOW_HOURS", "-3")
                .starts_with("ORDER_PAYMENT_WINDOW_HOURS is not a whole number of 0 or more"),
        );
        assert!(
            malformed("ORDER_PAYMENT_LIVE", "secret-switch")
                .starts_with("ORDER_PAYMENT_LIVE is not `on` or `off`"),
        );
        assert!(
            malformed("ORDER_PAYMENT_CURRENCY", "secret-currency")
                .starts_with("ORDER_PAYMENT_CURRENCY is not a currency code"),
        );
        assert!(
            malformed("ORDER_PAYMENT_LISTEN", "secret-address")
                .starts_with("ORDER_PAYMENT_LISTEN is not an address"),
        );
    }

    #[derive(Debug, sofie_macros::Config)]
    struct RefundConfig {
        refund: Option<Payment>,
        refund_days: Option<u64>,
    }

    #[test]
    fn a_key_sharing_an_optional_group_s_prefix_leaves_the_group_unset() {
        let config: RefundConfig = Environment::of(&[("ORDER_REFUND_DAYS", "14")])
            .read("ORDER")
            .unwrap();

        assert!(config.refund.is_none());
        assert_eq!(config.refund_days, Some(14));
    }

    #[derive(Debug, sofie_macros::Config)]
    struct UnsetConfig {
        never_set_key: Option<String>,
    }

    #[test]
    fn a_handler_reads_the_config_loaded_at_boot() {
        assert!(loaded_config::<UnsetConfig>().is_err());

        load::<UnsetConfig>("sofie_config_test").unwrap();
        load::<UnsetConfig>("sofie_config_test").unwrap();

        let first = loaded_config::<UnsetConfig>().unwrap();
        let second = loaded_config::<UnsetConfig>().unwrap();
        assert!(std::ptr::eq(first.0, second.0));
        assert_eq!(first.never_set_key, None);
    }

    #[test]
    fn the_derive_describes_each_key() {
        let described: Vec<_> = OrderConfig::FIELDS
            .iter()
            .map(|field| (field.name, field.key, field.optional, field.fallback))
            .collect();

        assert_eq!(
            described,
            [
                ("invoice_prefix", "INVOICE_PREFIX", false, None),
                ("payment_window_hours", "PAYMENT_WINDOW_HOURS", true, None),
                ("api_key", "API_KEY", false, None),
                ("database", "DATABASE", false, Some("DATABASE")),
                ("queue", "QUEUE", false, Some("DATABASE")),
                ("payment", "PAYMENT", false, None),
                ("refund", "REFUND", true, None),
            ]
        );
        assert!(matches!(OrderConfig::FIELDS[1].kind, Kind::Number));
        assert!(matches!(OrderConfig::FIELDS[2].kind, Kind::Text));
        let Kind::Group(payment) = OrderConfig::FIELDS[6].kind else {
            panic!("a nested struct is a group");
        };
        let kinds: Vec<_> = payment.iter().map(|field| field.kind).collect();
        assert!(matches!(
            kinds[..],
            [Kind::Value, Kind::Address, Kind::Switch]
        ));
    }

    mod broker {
        #[derive(Debug, sofie_macros::Config)]
        pub(super) struct Queue {
            pub(super) url: String,
        }
    }

    #[derive(Debug, sofie_macros::Config)]
    struct BrokerConfig {
        queue: broker::Queue,
    }

    #[test]
    fn only_the_framework_s_own_database_keys_fall_back_to_the_app_s() {
        assert_eq!(BrokerConfig::FIELDS[0].fallback, None);
        let error = Environment::of(&[("DATABASE_URL", "postgres://app")])
            .read::<BrokerConfig>("ORDER")
            .unwrap_err();
        assert_eq!(error.to_string(), "ORDER_QUEUE_URL is not set");
        let config = Environment::of(&[("ORDER_QUEUE_URL", "amqp://broker")])
            .read::<BrokerConfig>("ORDER")
            .unwrap();
        assert_eq!(config.queue.url, "amqp://broker");
    }
}
