use std::sync::Arc;

use http::StatusCode;
use sofie_core::{Error, Result};
use sofie_db::{Db, Resolve};

use crate::answer::{self, Response};
use crate::caller::Caller;
use crate::routes::Route;
use crate::session::RequestSession;
use crate::target::Target;
use crate::{Access, Area, AreaMembers, RequestContext};

/// Derived by `sofie gen`, never written by hand; a `*` axis stands for every value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Permission(&'static str);

impl Permission {
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        self.0
    }
}

#[must_use]
pub fn is_permission(text: &str) -> bool {
    let axis = |part: &str| {
        part == "*"
            || (part.starts_with(|first: char| first.is_ascii_lowercase())
                && part.chars().all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
                }))
    };
    text.split('.').count() == 4 && text.split('.').all(axis)
}

/// A needed `*` (the whole record) is covered only by a `*`.
fn covers(grant: &str, needed: &str) -> bool {
    let mut needed_axes = needed.split('.');
    let covered = grant.split('.').all(|axis| {
        needed_axes
            .next()
            .is_some_and(|needed| axis == "*" || axis == needed)
    });
    covered && needed_axes.next().is_none()
}

pub struct RoutePermission {
    pub method: &'static str,
    pub path: &'static str,
    pub permission: Permission,
}

/// Nothing outside a `Permitted` area.
#[derive(Clone, Default, Hash)]
pub struct Permissions {
    granted: Arc<[String]>,
    /// A bearer token's, narrowing what its principal was granted.
    #[cfg(feature = "tokens")]
    scopes: Option<Arc<[String]>>,
}

impl Permissions {
    #[must_use]
    pub fn can(&self, permission: Permission) -> bool {
        self.can_named(permission.name())
    }

    #[must_use]
    pub fn can_named(&self, permission: &str) -> bool {
        let granted = self.holds(permission);
        #[cfg(all(feature = "toolbar", debug_assertions))]
        record_check(permission, granted);
        granted
    }

    /// Unrecorded: the route's own check is a field of the request's span.
    fn holds(&self, permission: &str) -> bool {
        let granted = self.granted.iter().any(|grant| covers(grant, permission));
        #[cfg(feature = "tokens")]
        let granted = granted
            && self
                .scopes
                .as_ref()
                .is_none_or(|scopes| scopes.iter().any(|scope| covers(scope, permission)));
        granted
    }
}

/// A check a handler or screen makes shows in the development toolbar's span tree.
#[cfg(all(feature = "toolbar", debug_assertions))]
fn record_check(permission: &str, granted: bool) {
    let outcome = if granted { "granted" } else { "refused" };
    drop(tracing::info_span!("permission", otel.name = permission, permission = outcome).entered());
}

impl<A: Area, M> RequestContext<A, M> {
    /// For a check the route's permission cannot make, such as granting no more than one holds.
    #[must_use]
    pub fn permissions(&self) -> &Permissions {
        &self.matched.gates.permissions
    }
}

impl<A: Area, M> Resolve<RequestContext<A, M>> for Permissions {
    fn resolve(context: &mut RequestContext<A, M>) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Ok(context.permissions().clone()))
    }
}

#[derive(Default)]
pub(crate) struct Permits {
    members: Option<Arc<dyn AreaMembers>>,
    table: &'static [RoutePermission],
}

impl crate::Gates {
    #[must_use]
    pub fn route_permissions(mut self, table: &'static [RoutePermission]) -> Self {
        self.permits.table = table;
        self
    }
}

/// For a framework crate's own routes, which `sofie gen` does not read.
pub trait NeedsPermission {
    fn permission(&mut self, permission: Permission) -> &mut Self;
}

impl<A: Area> NeedsPermission for crate::PageRoute<'_, A> {
    fn permission(&mut self, permission: Permission) -> &mut Self {
        self.rule(|rules| rules.permission = Some(permission))
    }
}

impl<A: Area> NeedsPermission for crate::ActionRoute<'_, A> {
    fn permission(&mut self, permission: Permission) -> &mut Self {
        self.rule(|rules| rules.permission = Some(permission))
    }
}

impl Permits {
    pub(crate) async fn open(
        &self,
        target: &Target,
        session: &RequestSession,
        db: &Db,
        caller: Caller<'_>,
    ) -> Result<Permissions, Box<Response>> {
        let audience = target.audience;
        let principal = session.principal_in(audience.name);
        let (Some(principal), Some(members), Access::Permitted) =
            (principal, &self.members, audience.access)
        else {
            return Ok(Permissions::default());
        };
        let granted = members
            .grants(&db.read(), audience.name, principal)
            .await
            .map_err(|error| Box::new(answer::failure(error, caller)))?;
        let permissions = Permissions {
            granted: granted.into(),
            #[cfg(feature = "tokens")]
            scopes: session.scopes(),
        };
        let needed = target.admission.rules.permission;
        if needed.is_some_and(|needed| permissions.holds(needed.name())) {
            tracing::Span::current().record("permission", "granted");
            return Ok(permissions);
        }
        tracing::Span::current().record("permission", "refused");
        tracing::warn!(
            http.route = target.pattern.as_str(),
            area = audience.name,
            "refused a principal without the route's permission"
        );
        let refusal = answer::refusal(StatusCode::FORBIDDEN, "You may not do this.", caller);
        Err(Box::new(refusal))
    }

    /// Every route of a `Permitted` area must need a permission, or nobody could be let in by rule.
    pub(crate) fn prepare(
        &mut self,
        routes: &mut [Route],
        members: Option<&Arc<dyn AreaMembers>>,
    ) -> Result<()> {
        self.members = members.cloned();
        #[cfg(feature = "tokens")]
        refuse_unscoped_tokens(routes)?;
        refuse_unread_permissions(routes)?;
        for route in routes
            .iter_mut()
            .filter(|route| route.audience.access == Access::Permitted)
        {
            let listed = self
                .table
                .iter()
                .find(|entry| entry.method == route.method.as_str() && entry.path == route.path)
                .map(|entry| entry.permission);
            let permission = route.rules.permission.or(listed).ok_or_else(|| {
                Error::internal_message(format!(
                    "route {} {} of area `{}` needs a permission, but the wiring names none; run `sofie gen`",
                    route.method, route.path, route.audience.name
                ))
            })?;
            if self.members.is_none() {
                return Err(Error::internal_message(format!(
                    "area `{}` admits by permission, but the app has no members to hold grants; give `App::membership` the identity's memberships",
                    route.audience.name
                )));
            }
            route.rules.permission = Some(permission);
        }
        Ok(())
    }
}

/// Only a `Permitted` area reads grants, so a permission named elsewhere would check nobody.
fn refuse_unread_permissions(routes: &[Route]) -> Result<()> {
    let unread = routes.iter().find_map(|route| {
        let permission = route.rules.permission?;
        (route.audience.access != Access::Permitted).then_some((route, permission))
    });
    let Some((route, permission)) = unread else {
        return Ok(());
    };
    Err(Error::internal_message(format!(
        "route {} {} needs `{}`, but area `{}` does not admit by permission, so no grant would be checked; mount it in a `Permitted` area",
        route.method,
        route.path,
        permission.name(),
        route.audience.name
    )))
}

/// A token's scopes narrow grants, which only a `Permitted` area reads.
#[cfg(feature = "tokens")]
fn refuse_unscoped_tokens(routes: &[Route]) -> Result<()> {
    let unscoped = routes
        .iter()
        .find(|route| route.audience.tokens && route.audience.access != Access::Permitted);
    match unscoped {
        Some(route) => Err(Error::internal_message(format!(
            "area `{}` takes bearer tokens, but only a `Permitted` area holds a token to its scopes",
            route.audience.name
        ))),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_covers_what_matches_it_axis_by_axis() {
        assert!(covers("order.order.*.cancel", "order.order.*.cancel"));
        assert!(covers("order.*.*.*", "order.order.total.read"));
        assert!(covers("*.*.*.*", "order.order.*.cancel"));
        assert!(!covers("order.order.total.cancel", "order.order.*.cancel"));
        assert!(!covers("order.order.*.read", "order.order.*.cancel"));
        assert!(!covers("order.order.*", "order.order.*.read"));
    }

    #[cfg(all(feature = "toolbar", debug_assertions))]
    #[test]
    fn a_check_made_in_a_handler_is_recorded_for_the_toolbar() {
        let telemetry = sofie_telemetry::capture();
        let permissions = Permissions {
            granted: Arc::from(["order.*.*.*".to_owned()]),
            #[cfg(feature = "tokens")]
            scopes: None,
        };

        assert!(permissions.can(Permission::new("order.order.*.cancel")));
        assert!(!permissions.can_named("user.user.*.read"));

        let checks: Vec<(String, String)> = telemetry
            .spans()
            .into_iter()
            .filter(|span| span.name == "permission")
            .map(|span| {
                (
                    span.fields["otel.name"].clone(),
                    span.fields["permission"].clone(),
                )
            })
            .collect();
        assert_eq!(
            checks,
            [
                ("order.order.*.cancel".to_owned(), "granted".to_owned()),
                ("user.user.*.read".to_owned(), "refused".to_owned()),
            ]
        );
    }

    #[test]
    fn a_permission_has_four_named_or_wildcard_axes() {
        assert!(is_permission("order.order.*.cancel"));
        assert!(is_permission("*.*.*.*"));
        for refused in [
            "order.order.cancel",
            "order.Order.*.read",
            "order..*.read",
            "a.b.c.d.e",
            "order.order.**.read",
        ] {
            assert!(!is_permission(refused), "{refused}");
        }
    }
}
