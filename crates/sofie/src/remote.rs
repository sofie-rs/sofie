//! Every call is signed with `SOFIE_SIGNING_KEYS`, which all apps of a deployment share.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use sofie_client::Vendor;
use sofie_core::{Error, Result, unpoisoned};
use sofie_db::FeedSource;

use crate::client::read_keys;
use crate::config::environment::Environment;

pub use sofie_client::{Remote, RemoteApp, RemoteFeed, RemoteLookup};
pub use sofie_db::remote::{acting, acting_as};
pub use sofie_http::remote::{Serve, Served, answered, arguments, encoded, unknown_method};

/// Connected once per process, so one table serves every app a process builds.
fn connected() -> &'static RwLock<HashMap<&'static str, RemoteApp>> {
    static CONNECTED: OnceLock<RwLock<HashMap<&'static str, RemoteApp>>> = OnceLock::new();
    CONNECTED.get_or_init(RwLock::default)
}

/// A test process connects nowhere: a test provides the contract's fake itself.
pub fn connect(module: &'static str) -> Result<()> {
    if crate::fakes::provided_by_tests() {
        return Ok(());
    }
    let app = connection(&Environment::of_process(), module)?;
    unpoisoned::write(connected()).insert(module, app);
    Ok(())
}

fn connection(environment: &Environment, module: &'static str) -> Result<RemoteApp> {
    let prefix = format!("{}_REMOTE", module.to_ascii_uppercase());
    let vendor: Vendor = read_keys(environment, &prefix)?;
    if vendor.base_url.is_none() {
        return Err(Error::internal_message(format!(
            "{prefix}_BASE_URL is not set; it names the app module `{module}` was split into"
        )));
    }
    let keys = sofie_auth::SigningKeys::from_environment()?;
    Ok(RemoteApp::new(
        module,
        sofie_client::for_vendor(module, vendor)?,
        keys,
    ))
}

#[must_use]
pub fn app(module: &str) -> Option<RemoteApp> {
    let connected = unpoisoned::read(connected());
    connected.get(module).cloned()
}

/// For `Feeds::pull`: nothing in a test process, which connects nowhere.
#[must_use]
pub fn feed(module: &str) -> Option<Arc<dyn FeedSource>> {
    app(module).map(|app| Arc::new(RemoteFeed::new(app)) as Arc<dyn FeedSource>)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_split_module_is_reached_through_its_remote_keys() {
        let environment = Environment::of(&[]);

        let error = connection(&environment, "order").err().unwrap().to_string();

        assert_eq!(
            error,
            "ORDER_REMOTE_BASE_URL is not set; it names the app module `order` was split into"
        );
    }
}
