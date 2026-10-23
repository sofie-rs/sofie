use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::time::Duration;

use sofie_core::{DisplayZone, Error, Result, environment_variable};
use sofie_db::PoolSettings;

const DATABASE_URL: &str = "DATABASE_URL";
const ADDRESS: &str = "SOFIE_ADDRESS";
pub(crate) const TRUSTED_PROXIES: &str = "SOFIE_TRUSTED_PROXIES";
pub(crate) const REQUEST_ID_HEADER: &str = "SOFIE_REQUEST_ID_HEADER";
const MIGRATE: &str = "SOFIE_MIGRATE";
const RUNTIME_THREADS: &str = "SOFIE_RUNTIME_THREADS";
const POOL_SIZE: &str = "SOFIE_DB_POOL_SIZE";
const POOL_WAIT: &str = "SOFIE_DB_POOL_WAIT_MS";
const WORKER_BATCH: &str = "SOFIE_WORKER_BATCH";
const WORKER_POOL_SIZE: &str = "SOFIE_WORKER_POOL_SIZE";
const TIME_ZONE: &str = "SOFIE_TIME_ZONE";
/// The driver's own default: twice the logical CPUs.
const CONNECTIONS_PER_THREAD: NonZeroUsize = NonZeroUsize::new(2).unwrap();
/// Local only until the deployment says otherwise.
const DEFAULT_ADDRESS: &str = "127.0.0.1:8080";

pub(crate) struct Settings {
    pub(crate) database_url: String,
    pub(crate) address: SocketAddr,
    /// Off unless set to `on`: which process migrates, and when, is the
    /// deployment's choice.
    pub(crate) migrate: bool,
    pub(crate) runtime_threads: Option<NonZeroUsize>,
    pub(crate) pool: PoolSettings,
    pub(crate) worker_batch: Option<NonZeroUsize>,
    /// Its own pool, so jobs cannot starve requests of connections.
    pub(crate) worker_pool: Option<NonZeroUsize>,
}

impl Settings {
    pub(crate) fn from_environment() -> Result<Self> {
        Self::read(environment_variable)
    }

    fn read(text: impl Fn(&str) -> Result<Option<String>>) -> Result<Self> {
        let database_url = text(DATABASE_URL)?
            .ok_or_else(|| Error::internal_message(format!("{DATABASE_URL} is not set")))?;
        let address = text(ADDRESS)?.unwrap_or_else(|| DEFAULT_ADDRESS.to_owned());
        let address = address.parse().map_err(|error| {
            Error::internal(
                format!("{ADDRESS} `{address}` is not an address such as {DEFAULT_ADDRESS}"),
                error,
            )
        })?;
        #[cfg(not(feature = "trusted-proxies"))]
        for key in [TRUSTED_PROXIES, REQUEST_ID_HEADER] {
            if text(key)?.is_some() {
                return Err(Error::internal_message(format!(
                    "{key} is set, but the app trusts no proxy; enable sofie's `trusted-proxies` feature"
                )));
            }
        }
        let migrate = switch(MIGRATE, text(MIGRATE)?.as_deref())?;
        let count = |name: &str| positive_count(name, text(name)?.as_deref());
        let runtime_threads = count(RUNTIME_THREADS)?;
        let mut pool = PoolSettings {
            connections: count(POOL_SIZE)?.or_else(|| {
                runtime_threads.and_then(|threads| threads.checked_mul(CONNECTIONS_PER_THREAD))
            }),
            ..PoolSettings::default()
        };
        if let Some(milliseconds) = count(POOL_WAIT)? {
            pool.wait =
                Duration::from_millis(u64::try_from(milliseconds.get()).unwrap_or(u64::MAX));
        }
        Ok(Self {
            database_url,
            address,
            migrate,
            runtime_threads,
            pool,
            worker_batch: count(WORKER_BATCH)?,
            worker_pool: count(WORKER_POOL_SIZE)?,
        })
    }

    pub(crate) fn worker_pool(&self) -> Option<PoolSettings> {
        let connections = self.worker_pool?;
        Some(PoolSettings {
            connections: Some(connections),
            ..self.pool
        })
    }
}

/// Once per start, after telemetry is up so the warning reaches the log.
pub(crate) fn warn_on_old_signing_keys() -> Result<()> {
    if environment_variable(sofie_auth::OLD_SIGNING_KEYS)?.is_some() {
        tracing::warn!(
            key = sofie_auth::OLD_SIGNING_KEYS,
            replacement = sofie_auth::SIGNING_KEYS,
            "the signing keys are read under their old name; rename the key"
        );
    }
    Ok(())
}

/// Before anything reads an instant, which fixes the zone; UTC when unset.
pub(crate) fn install_display_zone() -> Result<()> {
    match display_zone(environment_variable(TIME_ZONE)?.as_deref())? {
        Some(zone) => zone.install(),
        None => Ok(()),
    }
}

fn display_zone(value: Option<&str>) -> Result<Option<DisplayZone>> {
    value
        .filter(|name| !name.trim().is_empty())
        .map(|name| DisplayZone::named(name).map_err(|error| Error::internal(TIME_ZONE, error)))
        .transpose()
}

fn positive_count(name: &str, value: Option<&str>) -> Result<Option<NonZeroUsize>> {
    value
        .map(|value| {
            value.trim().parse().map_err(|_| {
                Error::internal_message(format!("{name} `{value}` is not a whole number above 0"))
            })
        })
        .transpose()
}

pub(crate) fn switch(name: &str, value: Option<&str>) -> Result<bool> {
    let Some(value) = value else {
        return Ok(false);
    };
    switched(value).map_err(|no_switch| {
        Error::internal_message(format!(
            "{name} `{}` is not `on` or `off`; {no_switch}",
            value.trim()
        ))
    })
}

/// Every switch, `SOFIE_*` or a module's own key, reads both spellings alike.
pub(crate) fn switched(value: &str) -> std::result::Result<bool, NoSwitch> {
    match value.trim() {
        "on" | "true" => Ok(true),
        "off" | "false" => Ok(false),
        _ => Err(NoSwitch),
    }
}

#[derive(Debug)]
pub(crate) struct NoSwitch;

impl std::fmt::Display for NoSwitch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("`true` and `false` also do")
    }
}

/// The value `variables` give `name`, as the environment would in a test.
#[cfg(test)]
pub(crate) fn given(variables: &[(&str, &str)], name: &str) -> Option<String> {
    variables
        .iter()
        .find(|(variable, _)| *variable == name)
        .map(|(_, value)| (*value).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(variables: &[(&str, &str)]) -> Result<Settings> {
        Settings::read(|name| Ok(crate::settings::given(variables, name)))
    }

    #[test]
    fn the_address_defaults_to_local_and_the_database_is_required() {
        let settings = read(&[(DATABASE_URL, "postgres://db.test/shop")]).unwrap();
        assert_eq!(settings.database_url, "postgres://db.test/shop");
        assert_eq!(settings.address.to_string(), DEFAULT_ADDRESS);

        let missing = read(&[]).err().unwrap();
        assert_eq!(missing.to_string(), "DATABASE_URL is not set");
    }

    #[test]
    fn an_address_is_read_and_a_malformed_one_refused() {
        let database = (DATABASE_URL, "postgres://db.test/shop");
        let settings = read(&[database, (ADDRESS, "0.0.0.0:80")]).unwrap();
        assert_eq!(settings.address.to_string(), "0.0.0.0:80");

        let error = read(&[database, (ADDRESS, "everywhere")]).err().unwrap();
        assert!(
            error.to_string().starts_with("SOFIE_ADDRESS `everywhere`"),
            "{error}"
        );
    }

    #[cfg(not(feature = "trusted-proxies"))]
    #[test]
    fn trusted_proxies_are_refused_without_their_feature() {
        let database = (DATABASE_URL, "postgres://db.test/shop");
        let error = read(&[database, (TRUSTED_PROXIES, "10.0.0.0/8")])
            .err()
            .unwrap();
        assert!(error.to_string().starts_with(TRUSTED_PROXIES), "{error}");
    }

    #[test]
    fn migrating_on_start_is_off_unless_turned_on() {
        let database = (DATABASE_URL, "postgres://db.test/shop");
        assert!(!read(&[database]).unwrap().migrate);
        assert!(!read(&[database, (MIGRATE, "off")]).unwrap().migrate);
        assert!(read(&[database, (MIGRATE, "on")]).unwrap().migrate);
        assert!(read(&[database, (MIGRATE, "true")]).unwrap().migrate);
        assert!(!read(&[database, (MIGRATE, "false")]).unwrap().migrate);

        let error = read(&[database, (MIGRATE, "yes")]).err().unwrap();
        assert_eq!(
            error.to_string(),
            "SOFIE_MIGRATE `yes` is not `on` or `off`; `true` and `false` also do"
        );
    }

    #[test]
    fn the_display_zone_is_utc_unless_named_and_an_unknown_one_is_refused() {
        assert!(display_zone(None).unwrap().is_none());
        assert!(display_zone(Some(" ")).unwrap().is_none());
        let zone = display_zone(Some("Europe/Istanbul")).unwrap().unwrap();
        assert_eq!(zone.name(), "Europe/Istanbul");

        let error = display_zone(Some("Mars/Olympus")).err().unwrap();
        assert!(
            error
                .to_string()
                .starts_with("SOFIE_TIME_ZONE: `Mars/Olympus`"),
            "{error}"
        );
    }

    #[test]
    fn tuning_counts_default_when_unset_and_must_be_above_zero() {
        let database = (DATABASE_URL, "postgres://db.test/shop");
        let defaults = read(&[database]).unwrap();
        assert_eq!(defaults.runtime_threads, None);
        assert_eq!(defaults.pool, PoolSettings::default());
        assert_eq!(defaults.worker_batch, None);
        assert_eq!(defaults.worker_pool(), None);

        let tuned = read(&[
            database,
            (RUNTIME_THREADS, "4"),
            (POOL_SIZE, "30"),
            (POOL_WAIT, "250"),
            (WORKER_BATCH, "32"),
            (WORKER_POOL_SIZE, "6"),
        ])
        .unwrap();
        assert_eq!(tuned.runtime_threads, NonZeroUsize::new(4));
        assert_eq!(tuned.pool.connections, NonZeroUsize::new(30));
        assert_eq!(tuned.pool.wait, Duration::from_millis(250));
        assert_eq!(tuned.worker_batch, NonZeroUsize::new(32));
        let worker_pool = tuned.worker_pool().unwrap();
        assert_eq!(worker_pool.connections, NonZeroUsize::new(6));
        assert_eq!(worker_pool.wait, Duration::from_millis(250));

        let threads_only = read(&[database, (RUNTIME_THREADS, "4")]).unwrap();
        assert_eq!(threads_only.pool.connections, NonZeroUsize::new(8));

        for refused in ["0", "-1", "many"] {
            let error = read(&[database, (POOL_SIZE, refused)]).err().unwrap();
            assert_eq!(
                error.to_string(),
                format!("SOFIE_DB_POOL_SIZE `{refused}` is not a whole number above 0")
            );
        }
    }
}
