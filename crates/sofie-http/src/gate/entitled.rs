use std::sync::Arc;

use http::StatusCode;
use sofie_core::{Result, Uuid};
use sofie_db::{BoxFuture, Db, Read};

use super::Gates;
use crate::Access;
use crate::answer::{self, Response};
use crate::caller::Caller;
#[cfg(feature = "engine")]
use crate::routes::View;
use crate::session::RequestSession;
use crate::target::Target;

/// Named `<module>.<entitlement>` like a flag; what grants one is the app's billing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entitlement(&'static str);

impl Entitlement {
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        self.0
    }
}

/// What a principal's plan holds, read once per request that needs it.
pub trait Entitled: Send + Sync + 'static {
    fn held<'a>(
        &'a self,
        read: &'a Read,
        area: &'static str,
        principal: Uuid,
    ) -> BoxFuture<'a, Result<Arc<[&'static str]>>>;
}

impl Gates {
    #[must_use]
    pub fn entitlements(mut self, source: impl Entitled) -> Self {
        self.entitled = Some(Arc::new(source));
        self
    }

    /// Read for a route needing an entitlement and for any page, whose screen or menu may ask.
    pub(super) async fn entitled(
        &self,
        target: &Target,
        session: &RequestSession,
        db: &Db,
        caller: Caller<'_>,
    ) -> Result<Arc<[&'static str]>, Box<Response>> {
        let needed = target.admission.rules.entitlement;
        let Some(source) = self
            .entitled
            .as_ref()
            .filter(|_| needed.is_some() || renders(target))
        else {
            return Ok(Arc::from([]));
        };
        let audience = target.audience;
        let held = match session.principal_in(audience.name) {
            Some(principal) => source
                .held(&db.read(), audience.name, principal)
                .await
                .map_err(|error| Box::new(answer::failure(error, caller)))?,
            // A signed-in area's guard sends the visitor to sign in first.
            None if audience.access != Access::Public => return Ok(Arc::from([])),
            None => Arc::from([]),
        };
        if let Some(needed) = needed
            && !held.contains(&needed.name())
        {
            tracing::info!(
                entitlement = needed.name(),
                http.route = target.pattern.as_str(),
                "refused a principal whose plan lacks the route's entitlement"
            );
            let refusal = answer::refusal(
                StatusCode::FORBIDDEN,
                "Your plan does not include this.",
                caller,
            );
            return Err(Box::new(refusal));
        }
        Ok(held)
    }
}

#[cfg(feature = "engine")]
fn renders(target: &Target) -> bool {
    target.page.is_some()
        || !matches!(
            target.view,
            View::Json
                | View::Document(_)
                | View::Redirect(_)
                | View::Follow
                | View::RedirectOrFollow(_)
        )
}

#[cfg(not(feature = "engine"))]
fn renders(_target: &Target) -> bool {
    false
}
