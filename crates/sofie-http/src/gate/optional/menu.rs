//! An entry is hidden from a viewer who could not follow it.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use sofie_core::{Error, Result};
use sofie_engine::push_escaped;

use crate::gate::RequestGates;
use crate::routes::Audience;
use crate::{App, Area, RoutePath};

pub struct Menu<A: Area> {
    entries: Vec<Entry>,
    area: PhantomData<fn() -> A>,
}

pub struct MenuEntry<'m> {
    #[cfg_attr(
        not(any(feature = "flags", feature = "entitlements")),
        expect(dead_code, reason = "only flags and entitlements modify it")
    )]
    entry: &'m mut Entry,
}

#[derive(Clone)]
struct Entry {
    label: String,
    route: RoutePath,
    #[cfg(feature = "flags")]
    features: Vec<sofie_flags::Feature>,
    #[cfg(feature = "permissions")]
    permission: Option<crate::Permission>,
    #[cfg(feature = "entitlements")]
    entitlements: Vec<crate::Entitlement>,
}

#[derive(Default)]
pub(crate) struct Menus(HashMap<&'static str, Vec<Entry>>);

impl<A: Area> Menu<A> {
    /// `route` must be a page of this area without parameters.
    pub fn entry(&mut self, label: &str, route: RoutePath) -> MenuEntry<'_> {
        let entry = self.entries.push_mut(Entry {
            label: label.to_owned(),
            route,
            #[cfg(feature = "flags")]
            features: Vec::new(),
            #[cfg(feature = "permissions")]
            permission: None,
            #[cfg(feature = "entitlements")]
            entitlements: Vec::new(),
        });
        MenuEntry { entry }
    }
}

#[cfg(feature = "flags")]
impl MenuEntry<'_> {
    pub fn feature(&mut self, feature: sofie_flags::Feature) -> &mut Self {
        self.entry.features.push(feature);
        self
    }
}

#[cfg(feature = "entitlements")]
impl MenuEntry<'_> {
    pub fn entitled(&mut self, entitlement: crate::Entitlement) -> &mut Self {
        self.entry.entitlements.push(entitlement);
        self
    }
}

pub trait Navigation {
    #[must_use]
    fn menu<A: Area>(self, define: impl FnOnce(&mut Menu<A>)) -> Self;
}

impl Navigation for App {
    fn menu<A: Area>(mut self, define: impl FnOnce(&mut Menu<A>)) -> Self {
        let mut menu = Menu {
            entries: Vec::new(),
            area: PhantomData,
        };
        define(&mut menu);
        let menus = Arc::get_mut(&mut self.gates.menus)
            .expect("menus are added while the app is built, before any request holds them");
        menus.0.entry(A::NAME).or_default().extend(menu.entries);
        self
    }
}

/// An entry no route of its area answers stops the app.
pub(crate) fn prepare(app: &mut App) -> Result<()> {
    let menus = Arc::get_mut(&mut app.gates.menus).expect("no request holds the menus yet");
    for (area, entries) in &mut menus.0 {
        for entry in entries {
            let route = app
                .routes
                .iter()
                .find(|route| {
                    route.audience.name == *area
                        && route.method == entry.route.method()
                        && route.path == entry.route.path()
                })
                .ok_or_else(|| {
                    Error::internal_message(format!(
                        "the menu of area `{area}` links to {} {}, which no page of the area \
                         without parameters answers",
                        entry.route.method(),
                        entry.route.path()
                    ))
                })?;
            #[cfg(feature = "flags")]
            entry.features.extend(route.rules.feature);
            #[cfg(feature = "entitlements")]
            entry.entitlements.extend(route.rules.entitlement);
            #[cfg(feature = "permissions")]
            {
                entry.permission = route
                    .rules
                    .permission
                    .filter(|_| route.audience.access == crate::Access::Permitted);
            }
            let _ = route;
        }
    }
    Ok(())
}

impl Menus {
    pub(crate) fn navigation(
        &self,
        area: &Audience,
        current: &str,
        gates: &RequestGates,
    ) -> String {
        let Some(entries) = self.0.get(area.name) else {
            return String::new();
        };
        let mut html = String::from("<nav class=\"sofie-nav\"><ul>");
        for entry in entries.iter().filter(|entry| entry.may_follow(area, gates)) {
            html.push_str("<li><a href=\"");
            push_escaped(&mut html, entry.route.path());
            html.push('"');
            if entry.route.path() == current {
                html.push_str(" aria-current=\"page\"");
            }
            html.push('>');
            push_escaped(&mut html, &entry.label);
            html.push_str("</a></li>");
        }
        html.push_str("</ul></nav>");
        html
    }
}

#[cfg(feature = "clients")]
impl Menus {
    pub(crate) fn entries(
        &self,
        area: &Audience,
        current: &str,
        gates: &RequestGates,
    ) -> serde_json::Value {
        let entries = self.0.get(area.name).map_or(&[][..], Vec::as_slice);
        entries
            .iter()
            .filter(|entry| entry.may_follow(area, gates))
            .map(|entry| {
                serde_json::json!({
                    "label": entry.label,
                    "path": entry.route.path(),
                    "current": entry.route.path() == current,
                })
            })
            .collect()
    }
}

impl Entry {
    #[cfg_attr(
        not(any(feature = "flags", feature = "permissions", feature = "entitlements")),
        expect(clippy::unused_self, reason = "only gates hide an entry")
    )]
    fn may_follow(&self, area: &Audience, gates: &RequestGates) -> bool {
        let _ = (area, gates);
        #[cfg(feature = "flags")]
        if !self.features.iter().all(|feature| {
            gates
                .features
                .as_ref()
                .is_some_and(|features| features.enabled(*feature))
        }) {
            return false;
        }
        #[cfg(feature = "entitlements")]
        if !self
            .entitlements
            .iter()
            .all(|entitlement| gates.entitled.contains(&entitlement.name()))
        {
            return false;
        }
        #[cfg(feature = "permissions")]
        if area.access == crate::Access::Permitted
            && !self
                .permission
                .is_some_and(|permission| gates.permissions.can(permission))
        {
            return false;
        }
        true
    }
}

#[cfg(all(test, feature = "flags"))]
mod tests {
    use super::*;
    use crate::{Method, Routes, Web};

    async fn page() -> Result<()> {
        Ok(())
    }

    #[test]
    fn an_entry_behind_a_feature_is_hidden_from_an_app_without_flags() {
        let mut routes = Routes::<Web>::new("shop");
        routes.get("/beta", page);
        let mut menu = Menu::<Web> {
            entries: Vec::new(),
            area: PhantomData,
        };
        menu.entry("Beta", RoutePath::new(Method::GET, "/beta".to_owned()))
            .feature(sofie_flags::Feature::new("shop.beta"));
        let menus = Menus(HashMap::from([(Web::NAME, menu.entries)]));

        let shown = menus.navigation(&routes.added[0].audience, "/", &RequestGates::default());

        assert!(!shown.contains("Beta"), "{shown}");
    }
}

#[cfg(test)]
mod ordering {
    use super::*;
    use crate::{Gates, Method, Web};

    #[test]
    fn gates_given_after_a_menu_keep_it() {
        let app = App::new(sofie_db::Db::new(sofie_testing::FakeDriver::default()))
            .menu::<Web>(|menu| {
                menu.entry("Home", RoutePath::new(Method::GET, "/".to_owned()));
            })
            .gates(Gates::default());

        assert_eq!(app.gates.menus.0[Web::NAME].len(), 1);
    }
}
