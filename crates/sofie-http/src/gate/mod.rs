//! Without `flags`, `permissions` and `entitlements` the gates are empty and let everything through.

#[cfg(feature = "entitlements")]
mod entitled;
#[cfg(feature = "flags")]
mod flags;
#[cfg(any(feature = "listing", feature = "menu", feature = "groups"))]
pub(crate) mod optional;
#[cfg(feature = "permissions")]
mod permit;

#[cfg(any(feature = "menu", feature = "entitlements"))]
use std::sync::Arc;

use sofie_core::Result;
use sofie_db::Db;
#[cfg(feature = "engine")]
use sofie_db::Reply;

#[cfg(feature = "entitlements")]
pub use entitled::{Entitled, Entitlement};
#[cfg(feature = "flags")]
pub use flags::{FlagRules, RouteFeature};
#[cfg(any(feature = "listing", feature = "menu", feature = "groups"))]
pub use optional::*;
#[cfg(feature = "permissions")]
pub use permit::{NeedsPermission, Permission, Permissions, RoutePermission, is_permission};

use crate::answer::Response;
use crate::caller::Caller;
use crate::session::RequestSession;
use crate::target::Target;

#[derive(Default)]
pub struct Gates {
    #[cfg(feature = "flags")]
    flags: flags::FlagGate,
    #[cfg(feature = "permissions")]
    pub(crate) permits: permit::Permits,
    #[cfg(feature = "menu")]
    pub(crate) menus: Arc<optional::menu::Menus>,
    #[cfg(feature = "entitlements")]
    entitled: Option<Arc<dyn Entitled>>,
}

#[derive(Clone, Default)]
pub(crate) struct RequestGates {
    #[cfg(feature = "flags")]
    pub(crate) features: Option<sofie_flags::Features>,
    #[cfg(feature = "flags")]
    pub(crate) flags: Option<sofie_flags::Flags>,
    #[cfg(feature = "permissions")]
    pub(crate) permissions: crate::Permissions,
    #[cfg(feature = "menu")]
    pub(crate) menus: Arc<optional::menu::Menus>,
    #[cfg(feature = "entitlements")]
    #[cfg_attr(
        not(feature = "engine"),
        expect(dead_code, reason = "only screens and menus read it")
    )]
    pub(crate) entitled: Arc<[&'static str]>,
}

impl Gates {
    #[cfg(all(feature = "flags", feature = "cors"))]
    pub(crate) fn hides_from_anyone(&self, target: &Target) -> bool {
        self.flags.hides_from_anyone(target)
    }

    #[cfg_attr(
        not(any(feature = "permissions", feature = "entitlements")),
        expect(
            clippy::unused_async_trait_impl,
            reason = "only grants and entitlements are read from the database"
        )
    )]
    pub(crate) async fn open(
        &self,
        target: &Target,
        session: &RequestSession,
        db: &Db,
        caller: Caller<'_>,
    ) -> Result<RequestGates, Box<Response>> {
        let _ = (target, session, db, caller);
        Ok(RequestGates {
            #[cfg(feature = "flags")]
            features: self.flags.open(target, session, caller)?,
            #[cfg(feature = "flags")]
            flags: self.flags.held(),
            #[cfg(feature = "permissions")]
            permissions: self.permits.open(target, session, db, caller).await?,
            #[cfg(feature = "menu")]
            menus: Arc::clone(&self.menus),
            #[cfg(feature = "entitlements")]
            entitled: self.entitled(target, session, db, caller).await?,
        })
    }
}

/// Must run before the router is built.
#[cfg_attr(
    not(any(
        feature = "flags",
        feature = "permissions",
        feature = "resources",
        feature = "menu"
    )),
    expect(
        clippy::unnecessary_wraps,
        reason = "only flags, permissions, resources and menus are prepared"
    )
)]
pub(crate) fn prepare(app: &mut crate::App) -> Result<()> {
    let _ = &app;
    #[cfg(feature = "flags")]
    app.gates.flags.prepare(&mut app.routes)?;
    #[cfg(feature = "permissions")]
    app.gates
        .permits
        .prepare(&mut app.routes, app.members.as_ref())?;
    #[cfg(any(feature = "resources", feature = "menu"))]
    optional::prepare(app)?;
    Ok(())
}

impl RequestGates {
    /// An action refreshing another route's page, or showing it refused, shows only what that
    /// page's own gates would.
    #[cfg(feature = "engine")]
    #[cfg_attr(
        not(any(feature = "flags", feature = "permissions", feature = "entitlements")),
        expect(clippy::unused_self, reason = "no gate is compiled in")
    )]
    pub(crate) fn admit(&self, page: &crate::target::ScreenRoute) -> bool {
        let _ = page;
        #[cfg(feature = "flags")]
        if let Some(feature) = page.feature
            && !self
                .features
                .as_ref()
                .is_some_and(|features| features.enabled(feature))
        {
            return false;
        }
        #[cfg(feature = "permissions")]
        if let Some(permission) = page.permission
            && !self.permissions.can(permission)
        {
            return false;
        }
        #[cfg(feature = "entitlements")]
        if let Some(entitlement) = page.entitlement
            && !self.entitled.contains(&entitlement.name())
        {
            return false;
        }
        true
    }

    #[cfg_attr(
        not(feature = "flags"),
        expect(clippy::unused_self, reason = "no flags are compiled in")
    )]
    pub(crate) fn record(&self) {
        #[cfg(feature = "flags")]
        flags::record(self.features.as_ref());
    }
}

/// Answered as the routes were: nothing outside a `Permitted` area.
#[cfg(feature = "engine")]
impl sofie_engine::rendering::Gates for RequestGates {
    fn can(&self, permission: &str) -> bool {
        #[cfg(feature = "permissions")]
        return self.permissions.can_named(permission);
        #[cfg(not(feature = "permissions"))]
        {
            let _ = permission;
            false
        }
    }

    fn enabled(&self, flag: &str) -> bool {
        #[cfg(feature = "flags")]
        return self
            .features
            .as_ref()
            .is_some_and(|features| features.enabled_named(flag));
        #[cfg(not(feature = "flags"))]
        {
            let _ = flag;
            false
        }
    }

    #[cfg(feature = "entitlements")]
    fn entitled(&self, entitlement: &str) -> bool {
        self.entitled.contains(&entitlement)
    }
}

#[cfg(feature = "engine")]
impl crate::screen::ScreenPage<'_> {
    pub(crate) fn rendering(&self) -> Gated<'_> {
        let gates = self.answer.gates;
        #[cfg(feature = "i18n")]
        let rendering = self.language.screen(self.template).gates(gates);
        #[cfg(not(feature = "i18n"))]
        let rendering = sofie_engine::rendering::Rendering::new(self.template).gates(gates);
        Gated {
            rendering,
            #[cfg(feature = "menu")]
            navigation: gates.menus.navigation(
                &self.answer.target.audience,
                self.answer.asked.request.uri.path(),
                gates,
            ),
        }
    }
}

#[cfg(feature = "engine")]
impl crate::screen::ScreenPage<'_> {
    /// Answered as the page would be shown: without what this viewer is shown nothing of.
    pub(crate) fn withheld(&self, reply: Result<Reply>) -> Result<Reply> {
        let withheld = self.withheld_paths();
        if withheld.is_empty() || matches!(reply, Ok(Reply::Empty)) {
            return reply;
        }
        let mut data = reply?.into_json()?;
        withheld.apply_to(&mut data);
        Ok(Reply::Json(data))
    }

    #[cfg(feature = "clients")]
    pub(crate) fn withhold(&self, data: &mut serde_json::Value) {
        self.withheld_paths().apply_to(data);
    }

    fn withheld_paths(&self) -> sofie_engine::rendering::Withheld<'_> {
        sofie_engine::rendering::Rendering::new(self.template)
            .gates(self.answer.gates)
            .withheld()
    }
}

#[cfg(feature = "clients")]
impl crate::screen::ScreenPage<'_> {
    /// The area's menu as this viewer may follow it.
    #[cfg_attr(
        not(feature = "menu"),
        expect(clippy::unused_self, reason = "only an app with menus has entries")
    )]
    pub(crate) fn navigation_entries(&self) -> serde_json::Value {
        #[cfg(feature = "menu")]
        return self.answer.gates.menus.entries(
            &self.answer.target.audience,
            self.answer.asked.request.uri.path(),
            self.answer.gates,
        );
        #[cfg(not(feature = "menu"))]
        serde_json::Value::Array(Vec::new())
    }
}

#[cfg(feature = "engine")]
pub(crate) struct Gated<'a> {
    rendering: sofie_engine::rendering::Rendering<'a>,
    #[cfg(feature = "menu")]
    navigation: String,
}

#[cfg(feature = "engine")]
impl Gated<'_> {
    #[cfg(feature = "clients")]
    pub(crate) fn client_view(&self) -> sofie_engine::ClientView {
        self.rendering.serializable_client_view()
    }

    #[cfg(all(feature = "clients", feature = "listing"))]
    pub(crate) fn translate_listing_errors(&self, data: &mut serde_json::Value) {
        self.rendering.translate_listing_errors(data);
    }

    pub(crate) fn render(
        &self,
        data: &dyn sofie_core::Show,
    ) -> Result<String, sofie_engine::TemplateError> {
        self.rendering
            .render(data)
            .map(|body| self.under_menu(body))
    }

    pub(crate) fn render_submitted(
        &self,
        data: &dyn sofie_core::Show,
        submitted: &sofie_engine::Submitted<'_>,
    ) -> Result<String, sofie_engine::TemplateError> {
        self.rendering
            .render_submitted(data, submitted)
            .map(|body| self.under_menu(body))
    }

    pub(crate) fn render_fragment(
        &self,
        data: &dyn sofie_core::Show,
        id: &str,
    ) -> Result<String, sofie_engine::RenderError> {
        self.rendering.render_fragment(data, id)
    }

    pub(crate) fn render_out_of_band(
        &self,
        data: &dyn sofie_core::Show,
        id: &str,
    ) -> Result<String, sofie_engine::RenderError> {
        self.rendering.render_out_of_band(data, id)
    }

    #[cfg_attr(
        not(feature = "menu"),
        expect(clippy::unused_self, unused_mut, reason = "only a menu comes first")
    )]
    fn under_menu(&self, mut body: String) -> String {
        #[cfg(feature = "menu")]
        body.insert_str(0, &self.navigation);
        body
    }
}
