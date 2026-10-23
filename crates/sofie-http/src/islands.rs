use http::Method;
use sofie_core::{Error, Result};
use sofie_engine::islands::{IslandBundle, IslandLinks, is_island_name, island_screen};

use crate::handler::{Loader, loader_endpoint};
use crate::routes::{RouteRules, ScreenView, View};
use crate::{Area, PageRoute, Routes, Screens, screen_key};

pub(crate) struct IslandScreen {
    pub(crate) key: String,
    pub(crate) tag: String,
}

impl<A: Area> Routes<A> {
    /// A module wraps the island in a page with `screens/<name>.html` holding `<Island … />`.
    pub fn island<Args: 'static, H: Loader<A, Args>>(
        &mut self,
        path: &str,
        name: &str,
        loader: H,
    ) -> PageRoute<'_, A> {
        let key = screen_key(self.module, A::NAME, name);
        self.islands.push(IslandScreen {
            key: key.clone(),
            tag: name.to_owned(),
        });
        let view = View::Screen(ScreenView {
            key,
            title: sofie_engine::humanize(&name.replace('-', "_")),
            #[cfg(feature = "resources")]
            builtin: None,
        });
        PageRoute::new(self.push(
            Method::GET,
            path,
            loader_endpoint(loader),
            view,
            RouteRules::default(),
        ))
    }
}

#[derive(Default)]
pub(crate) struct Islands {
    pub(crate) bundles: Vec<IslandBundle>,
}

impl Islands {
    pub(crate) fn links(&self, area: &str) -> Option<IslandLinks> {
        #[cfg(all(feature = "development", debug_assertions))]
        if let Some(links) = crate::development::island_links(area) {
            return Some(links);
        }
        self.bundles
            .iter()
            .find(|bundle| bundle.area == area)
            .map(IslandLinks::of)
    }
}

impl crate::assets::Assets {
    pub(crate) fn with_islands(mut self, screens: &Screens) -> Self {
        for bundle in &screens.islands.bundles {
            for asset in [bundle.script, bundle.stylesheet] {
                self.served.push((asset.path(), asset));
            }
        }
        self
    }
}

impl Screens {
    pub fn islands(&mut self, bundles: Vec<IslandBundle>) -> &mut Self {
        self.islands.bundles = bundles;
        self
    }
}

pub(crate) fn install(screens: &mut Screens, islands: &[IslandScreen]) -> Result<()> {
    for island in islands {
        if !is_island_name(&island.tag) {
            return Err(Error::internal_message(format!(
                "screen {} names island `{}`, which is no custom element name: name \
                 `islands/<name>.svelte` in lowercase with a hyphen, such as `order-chart`",
                island.key, island.tag
            )));
        }
        if screens.contains(&island.key) {
            continue;
        }
        screens.add_with(&island.key, || {
            island_screen(&island.key, &island.tag)
                .map_err(|error| Error::internal(format!("building screen {}", island.key), error))
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_island_page_renders_the_island_unless_the_module_wraps_it() {
        let mut screens = crate::screens_from_sources(&[(
            "shop/web/wrapped",
            "<main><Island name=\"wrapped-chart\" props={data} /></main>",
        )])
        .unwrap();
        let islands = [
            IslandScreen {
                key: "shop/web/order-chart".to_owned(),
                tag: "order-chart".to_owned(),
            },
            IslandScreen {
                key: "shop/web/wrapped".to_owned(),
                tag: "wrapped-chart".to_owned(),
            },
        ];

        install(&mut screens, &islands).unwrap();

        let data = serde_json::json!({"count": 1});
        let page = screens
            .template("shop/web/order-chart")
            .unwrap()
            .render(&data)
            .unwrap();
        assert_eq!(
            page,
            "<order-chart><script type=\"application/json\">{\"count\":1}</script></order-chart>"
        );
        let wrapped = screens
            .template("shop/web/wrapped")
            .unwrap()
            .render(&data)
            .unwrap();
        assert!(wrapped.starts_with("<main><wrapped-chart>"), "{wrapped}");
    }

    #[test]
    fn an_island_needs_a_custom_element_name() {
        let error = install(
            &mut Screens::default(),
            &[IslandScreen {
                key: "shop/web/chart".to_owned(),
                tag: "chart".to_owned(),
            }],
        )
        .unwrap_err();

        assert!(
            error.to_string().contains("names island `chart`"),
            "{error}"
        );
    }

    #[test]
    fn a_page_loads_the_bundle_of_its_area_only() {
        let bundle = IslandBundle::new("member", b"export {};", b"");
        let islands = Islands {
            bundles: vec![bundle],
        };

        assert_eq!(islands.links("member"), Some(IslandLinks::of(&bundle)));
        assert_eq!(islands.links("console"), None);
    }
}
