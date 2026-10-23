use std::fmt::Write as _;
use std::sync::Arc;

use sofie::db::{Context, Db, Resolve};
use sofie::engine::push_escaped;
use sofie::http::{Area, ListedRoute, Media, Rate, ReadContext, Text, path_segment};
use sofie::prelude::*;
use sofie::values::{Slug, Url};

use crate::{Markdown, Page};

const PREFIX: &str = "/pages";
const BASE_URL: &str = "SOFIE_CONTENT_BASE_URL";
const DEVELOPMENT_URL: &str = "http://127.0.0.1:8080";
const SITEMAP_LIMIT: u64 = 50_000;
/// The sitemap is public and reads up to 50 000 rows, while a crawler fetches it rarely.
const SITEMAP_RATE: Rate = Rate::per_minute(6);

pub const ROUTES: &[ListedRoute] = &[
    ListedRoute::new("routes", "GET", "/pages/{slug}", "page"),
    ListedRoute::new("routes", "GET", "/sitemap.xml", "sitemap"),
];

pub fn routes(routes: &mut Routes<Web>) {
    routes
        .screen(&format!("{PREFIX}/{{slug}}"), "page", show_page)
        .name("page");
    routes
        .get("/sitemap.xml", sitemap)
        .name("sitemap")
        .document(Media::Xml)
        .throttle(SITEMAP_RATE);
}

#[derive(ReadModel)]
#[read(from = Page)]
struct Published {
    title: String,
    summary: String,
    body: Markdown,
    image: Option<Url>,
}

#[derive(ReadModel)]
struct Shown {
    page: Published,
    url: String,
}

#[derive(ReadModel)]
#[read(from = Page)]
struct Listed {
    slug: String,
    updated_at: Instant,
}

async fn show_page(read: Read, slug: PageSlug, site: Site) -> Result<Shown> {
    let page = Published::query()
        .where_(Page::PUBLISHED_AT.is_not_null())
        .by_slug(&read, slug.0.as_str())
        .await?;
    let url = site.0.page(slug.0.as_str());
    Ok(Shown { page, url })
}

async fn sitemap(read: Read, site: Site) -> Result<Text> {
    let pages = Listed::query()
        .where_(Page::PUBLISHED_AT.is_not_null())
        .latest()
        .limit(SITEMAP_LIMIT)
        .all(&read)
        .await?;
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    for page in pages {
        xml.push_str("<url><loc>");
        push_escaped(&mut xml, &site.0.page(&page.slug));
        let day = page.updated_at.timestamp().strftime("%Y-%m-%d");
        let _ = writeln!(xml, "</loc><lastmod>{day}</lastmod></url>");
    }
    xml.push_str("</urlset>\n");
    Ok(Text(xml))
}

struct PageSlug(Slug);

impl<A: Area> Resolve<ReadContext<A>> for PageSlug {
    fn resolve(context: &mut ReadContext<A>) -> impl Future<Output = Result<Self>> + Send {
        let slug = context
            .path_parameter("slug")
            .and_then(|text| Slug::parse(text).ok())
            .map(Self)
            .ok_or(Error::NotFound);
        std::future::ready(slug)
    }
}

pub struct Settings {
    base_url: String,
}

impl Settings {
    /// From `SOFIE_CONTENT_BASE_URL`, else `SOFIE_BASE_URL`, such as `https://example.com`;
    /// a debug build without either, as in tests and development, is reached at the default address.
    pub fn from_environment() -> Result<Self> {
        Self::read(sofie::core::environment_variable)
    }

    fn read(lookup: impl Fn(&str) -> Result<Option<String>>) -> Result<Self> {
        match sofie::core::base_url(BASE_URL, lookup)? {
            Some(variable) => Self::named(variable.name, &variable.value),
            None if cfg!(debug_assertions) => Self::new(DEVELOPMENT_URL),
            None => Err(sofie::core::base_url_missing(
                BASE_URL,
                "name where the app is reached, such as https://example.com",
            )),
        }
    }

    pub fn new(base_url: &str) -> Result<Self> {
        Self::named(BASE_URL, base_url)
    }

    fn named(name: &str, base_url: &str) -> Result<Self> {
        let url = Url::parse(base_url)
            .map_err(|error| Error::internal(format!("{name} `{base_url}`"), error))?;
        if url.as_str().contains(['?', '#']) {
            return Err(Error::internal_message(format!(
                "{name} `{base_url}` must not carry a query or a fragment"
            )));
        }
        Ok(Self {
            base_url: url.as_str().trim_end_matches('/').to_owned(),
        })
    }

    fn page(&self, slug: &str) -> String {
        format!("{}{PREFIX}/{}", self.base_url, path_segment(&slug))
    }
}

/// Called by the wiring at boot, so a missing base URL stops the app from starting.
pub fn install(db: &Db, settings: Settings) {
    sofie::db::provide(db, Arc::new(settings));
}

struct Site(Arc<Settings>);

impl<A: Area> Resolve<ReadContext<A>> for Site {
    fn resolve(context: &mut ReadContext<A>) -> impl Future<Output = Result<Self>> + Send {
        let settings = sofie::db::provided::<Settings>(context.db())
            .map(Self)
            .ok_or_else(|| {
                Error::internal_message(
                    "sofie-content is not installed: the wiring calls `sofie_content::install`",
                )
            });
        std::future::ready(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;

    #[test]
    fn the_base_url_is_an_http_address_without_its_last_slash() {
        let settings = Settings::new("https://example.com/").unwrap();

        assert_eq!(settings.page("about"), "https://example.com/pages/about");
        assert_eq!(
            settings.page("öğle"),
            "https://example.com/pages/%C3%B6%C4%9Fle"
        );
        assert!(Settings::new("ftp://example.com").is_err());
        assert!(Settings::new("example.com").is_err());
        assert!(Settings::new("https://example.com/?campaign=x").is_err());
        assert!(Settings::new("https://example.com/#top").is_err());
    }

    #[test]
    fn the_shared_base_url_serves_unless_content_has_its_own() {
        let given = |variables: &'static [(&'static str, &'static str)]| {
            Settings::read(move |name: &str| {
                Ok(variables
                    .iter()
                    .find(|(variable, _)| *variable == name)
                    .map(|(_, value)| (*value).to_owned()))
            })
        };
        let shared = given(&[("SOFIE_BASE_URL", "https://example.com")]).unwrap();
        assert_eq!(shared.page("about"), "https://example.com/pages/about");
        let own = given(&[
            ("SOFIE_BASE_URL", "https://example.com"),
            ("SOFIE_CONTENT_BASE_URL", "https://pages.example.com"),
        ])
        .unwrap();
        assert_eq!(own.page("about"), "https://pages.example.com/pages/about");
        let malformed = given(&[("SOFIE_BASE_URL", "example.com")]).err().unwrap();
        assert!(
            malformed
                .to_string()
                .starts_with("SOFIE_BASE_URL `example.com`"),
            "{malformed}"
        );
    }
}
