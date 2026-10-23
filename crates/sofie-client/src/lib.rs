mod breaker;
mod connect;
mod exchange;
mod guard;
#[cfg(feature = "meilisearch")]
mod meilisearch;
#[cfg(feature = "remote-contracts")]
mod remote;
mod request;
mod settings;
mod target;
mod telemetry;
#[cfg(feature = "testing")]
pub mod testing;

use std::sync::Arc;

use http::Method;
use sofie_core::{Error, Result};

use breaker::Breakers;
use connect::Connector;
use guard::Reach;
#[cfg(feature = "meilisearch")]
pub use meilisearch::Meilisearch;
#[cfg(feature = "remote-contracts")]
pub use remote::{Remote, RemoteApp, RemoteFeed, RemoteLookup};
pub use request::{Request, Response};
pub use settings::Vendor;
use settings::{Allowed, Limits};
use target::Target;
pub use telemetry::call;

pub trait Adapter: Sized {
    /// Names its keys, `<MODULE>_<VENDOR>_*`, and labels its spans and metrics.
    const VENDOR: &'static str;
    type Settings;

    fn new(http: Outgoing, settings: Self::Settings) -> Self;
}

#[derive(Debug)]
pub struct NoSettings {}

/// Clones share connections and breakers.
#[derive(Clone)]
pub struct Outgoing {
    shared: Arc<Shared>,
}

struct Shared {
    vendor: &'static str,
    base: Option<Target>,
    allowed: Allowed,
    limits: Limits,
    connector: Connector,
    /// Apart, so hosts an untrusted URL names cannot crowd out the vendor's.
    vendor_hosts: Breakers,
    untrusted_hosts: Breakers,
    operations: Breakers,
}

/// The wiring's constructor: not re-exported by the `sofie` facade, refused in modules by `sofie check`.
#[doc(hidden)]
pub fn for_vendor(vendor: &'static str, settings: Vendor) -> Result<Outgoing> {
    Outgoing::build(vendor, settings, Connector::trusting_public_roots())
}

impl Outgoing {
    fn build(vendor: &'static str, settings: Vendor, connector: Connector) -> Result<Self> {
        let (base, allowed, limits) = settings.read(vendor)?;
        Ok(Self {
            shared: Arc::new(Shared {
                vendor,
                base,
                allowed,
                limits,
                connector,
                vendor_hosts: Breakers::default(),
                untrusted_hosts: Breakers::default(),
                operations: Breakers::default(),
            }),
        })
    }

    #[must_use]
    pub fn get(&self, path: &str) -> Request<'_> {
        self.to_vendor(Method::GET, path)
    }

    #[must_use]
    pub fn post(&self, path: &str) -> Request<'_> {
        self.to_vendor(Method::POST, path)
    }

    #[must_use]
    pub fn put(&self, path: &str) -> Request<'_> {
        self.to_vendor(Method::PUT, path)
    }

    #[must_use]
    pub fn patch(&self, path: &str) -> Request<'_> {
        self.to_vendor(Method::PATCH, path)
    }

    #[must_use]
    pub fn delete(&self, path: &str) -> Request<'_> {
        self.to_vendor(Method::DELETE, path)
    }

    /// An absolute URL on one of the vendor's hosts (its `BASE_URL`'s and `HOSTS`), for a
    /// vendor whose endpoints live on several hosts, as an OAuth provider's do.
    #[must_use]
    pub fn get_url(&self, url: &str) -> Request<'_> {
        self.to_vendor_url(Method::GET, url)
    }

    #[must_use]
    pub fn post_url(&self, url: &str) -> Request<'_> {
        self.to_vendor_url(Method::POST, url)
    }

    fn to_vendor_url(&self, method: Method, url: &str) -> Request<'_> {
        let target = Target::parse(url).map_err(|problem| {
            Error::internal_message(format!(
                "a URL of {} is refused: {problem}",
                self.shared.vendor
            ))
        });
        Request::new(self, method, target, Reach::Vendor)
    }

    /// Public HTTPS hosts only, whatever the vendor allows, and no redirects.
    #[must_use]
    pub fn untrusted(&self) -> Untrusted<'_> {
        Untrusted { outgoing: self }
    }

    pub fn operation<T>(
        &self,
        name: &'static str,
        work: impl Future<Output = Result<T>>,
    ) -> impl Future<Output = Result<T>> {
        telemetry::operation(&self.shared, name, work)
    }

    fn to_vendor(&self, method: Method, path: &str) -> Request<'_> {
        let target = match &self.shared.base {
            Some(base) => base.join(path).map_err(|problem| {
                Error::internal_message(format!(
                    "the path of a request to {}: {problem}",
                    self.shared.vendor
                ))
            }),
            None => Err(Error::internal_message(format!(
                "{} has no base URL; set its `BASE_URL` key",
                self.shared.vendor
            ))),
        };
        Request::new(self, method, target, Reach::Vendor)
    }
}

pub struct Untrusted<'a> {
    outgoing: &'a Outgoing,
}

impl Untrusted<'_> {
    #[must_use]
    pub fn get(&self, url: &str) -> Request<'_> {
        self.to(Method::GET, url)
    }

    #[must_use]
    pub fn post(&self, url: &str) -> Request<'_> {
        self.to(Method::POST, url)
    }

    fn to(&self, method: Method, url: &str) -> Request<'_> {
        let target = Target::parse(url)
            .map_err(|problem| Error::BadRequest(format!("the URL is refused: {problem}")));
        Request::new(self.outgoing, method, target, Reach::Untrusted)
    }
}
