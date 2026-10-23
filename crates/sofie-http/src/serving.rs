use std::num::NonZeroUsize;
use std::sync::Arc;

use sofie_auth::SigningKeys;
use sofie_core::Result;

use crate::health::{Dependency, Readiness};
use crate::{Area, RequestContext};

const UNNAMED: &str = "sofie";

#[derive(Default)]
pub struct Serving {
    pub(crate) name: Option<&'static str>,
    pub(crate) max_connections: Option<NonZeroUsize>,
    pub(crate) keys: Option<SigningKeys>,
    pub(crate) dependencies: Vec<Dependency>,
    pub(crate) readiness: Readiness,
    #[cfg(feature = "trusted-proxies")]
    pub(crate) proxies: crate::TrustedProxies,
    #[cfg(feature = "metrics")]
    pub(crate) metrics_token: Option<crate::MetricsToken>,
    #[cfg(feature = "browser-telemetry")]
    pub(crate) browser_sample: crate::BrowserSample,
    #[cfg(feature = "shield")]
    pub(crate) shield: crate::Shield,
    #[cfg(feature = "cors")]
    pub(crate) cors_origins: crate::CorsOrigins,
    #[cfg(feature = "storage")]
    pub(crate) storage: Option<sofie_storage::Storage>,
    #[cfg(feature = "tls")]
    pub(crate) tls: Option<crate::Tls>,
    #[cfg(feature = "engine")]
    pub(crate) content_security: std::collections::HashMap<&'static str, crate::ContentSecurity>,
    #[cfg(feature = "engine")]
    pub(crate) content_security_everywhere: crate::ContentSecurity,
}

/// Given by `Serving::named`, `sofie` without one; for an authenticator's issuer and the like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppName(&'static str);

impl AppName {
    pub(crate) fn of_serving(serving: &Serving) -> Self {
        Self(serving.name.unwrap_or(UNNAMED))
    }

    #[must_use]
    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for AppName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl<A: Area, M> sofie_db::Resolve<RequestContext<A, M>> for AppName {
    fn resolve(context: &mut RequestContext<A, M>) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Ok(context.matched.app))
    }
}

impl Serving {
    /// Names the app's cookies, `__Host-<name>-session` (cookies do not separate by port), and
    /// the [`AppName`] handlers take.
    #[must_use]
    pub fn named(mut self, name: &'static str) -> Self {
        self.name = Some(name);
        self
    }

    /// Without them, serving reads `SOFIE_SIGNING_KEYS` and refuses to start when it is missing.
    #[must_use]
    pub fn signing_keys(mut self, keys: SigningKeys) -> Self {
        self.keys = Some(keys);
        self
    }

    /// Connections served at once, 4096 by default; keep it below `ulimit -n`.
    #[must_use]
    pub fn max_connections(mut self, limit: NonZeroUsize) -> Self {
        self.max_connections = Some(limit);
        self
    }

    /// Checked after the database and the queue; while it fails, the instance is not ready.
    #[must_use]
    pub fn dependency<F>(
        mut self,
        name: &'static str,
        check: impl Fn() -> F + Send + Sync + 'static,
    ) -> Self
    where
        F: Future<Output = Result<()>> + Send + 'static,
    {
        self.dependencies.push(Dependency {
            name,
            check: Arc::new(move || Box::pin(check())),
        });
        self
    }
}
