use std::fmt::Write as _;

use sofie_model::naming::path_parameters;
use sofie_model::{Aggregate, KnownArea, Method, Module, ReadModel, Route};

use super::read_model::ReadFields;
use crate::gen_command::GenError;

const NO_BACK_LINK: &str = "none";

pub(super) struct DetailScreen<'route> {
    pub screen: &'route str,
    /// Names the page's heading and tab; the screen's name humanized, as the tab's, when `None`.
    pub title: Option<&'route str>,
    pub back: Option<BackLink>,
    pub read: Option<&'route ReadFields>,
    /// A record's id is left off its page unless `fields` names it.
    pub shows_id: bool,
}

pub(super) struct BackLink {
    href: String,
    label: String,
}

/// A record's fields as label and value pairs, labelled as the engine humanizes a name.
pub(super) fn screen_source(screen: &DetailScreen) -> String {
    let mut body = String::new();
    let page_title = screen
        .title
        .map(|title| format!("<title>{title}</title>\n"))
        .unwrap_or_default();
    let heading = screen
        .title
        .map_or_else(|| sofie_engine::humanize(screen.screen), str::to_owned);
    if let Some(back) = &screen.back {
        let _ = writeln!(body, "  <a href=\"{}\">{}</a>", back.href, back.label);
    }
    let shown = shown_fields(screen);
    if !shown.is_empty() {
        let _ = write!(body, "  <dl>\n{shown}  </dl>\n");
    }
    format!("{page_title}<Section title=\"{heading}\">\n{body}</Section>\n")
}

fn shown_fields(screen: &DetailScreen) -> String {
    let mut shown = String::new();
    let fields = screen.read.into_iter().flat_map(|read| &read.fields);
    for field in fields.filter(|field| screen.shows_id || field.name != "id") {
        let value = if field.money {
            format!("<Money value={{data.{}}} />", field.name)
        } else if field.instant {
            format!("{{date(data.{})}}", field.name)
        } else {
            format!("{{data.{}}}", field.name)
        };
        let label = sofie_engine::humanize(&field.name);
        let _ = write!(shown, "    <dt>{label}</dt>\n    <dd>{value}</dd>\n");
    }
    shown
}

/// Literal text of the template, so it may not open markup or a binding, nor close the attribute.
pub(super) fn checked_title(title: &str) -> Result<&str, GenError> {
    let plain = !title.trim().is_empty()
        && !title
            .chars()
            .any(|character| character.is_control() || "\"<>{}&".contains(character));
    if !plain {
        return Err(GenError::Usage(format!(
            "`{title}` is not a page title: write plain text, without quotes, `<`, `>`, `{{`, \
             `}}` or `&`"
        )));
    }
    Ok(title)
}

/// The link back from a record's page, and the note saying why it was chosen.
pub(super) fn back_link(
    module: &Module,
    area: &KnownArea,
    path: &str,
    bound: Option<&Aggregate>,
    asked: Option<&str>,
) -> Result<(Option<BackLink>, Option<String>), GenError> {
    let routes = area_routes(module, area.name);
    match asked {
        Some(NO_BACK_LINK) => Ok((None, None)),
        Some(handler) => {
            let route = named_page(&routes, area.name, handler)?;
            let link = link_to(area, route);
            let note = format!(
                "back link to {} (`{handler}`), as `back` names it",
                link.href
            );
            Ok((Some(link), Some(note)))
        }
        None => Ok(found_back_link(module, area, &routes, path, bound)),
    }
}

fn found_back_link(
    module: &Module,
    area: &KnownArea,
    routes: &[&Route],
    path: &str,
    bound: Option<&Aggregate>,
) -> (Option<BackLink>, Option<String>) {
    let Some(aggregate) = bound else {
        return (None, None);
    };
    let pages: Vec<&Route> = routes
        .iter()
        .copied()
        .filter(|route| route.screen.is_some() && is_page(route))
        .collect();
    let parent = parent_path(path);
    let listings: Vec<(&Route, &ReadModel)> = pages
        .iter()
        .filter_map(|route| Some((*route, listed_model(module, route, aggregate)?)))
        .collect();
    let listing = listings
        .iter()
        .find(|(route, _)| Some(route.path.as_str()) == parent)
        .or(listings.first());
    if let Some((route, model)) = listing {
        let link = link_to(area, route);
        let note = format!(
            "back link to {} (`{}`): it lists `{}` through `{}`; `back` names another page or \
             `none`",
            link.href,
            handler_name(route),
            aggregate.name,
            model.name
        );
        return (Some(link), Some(note));
    }
    let plural = pages
        .iter()
        .find(|route| Some(route.path.as_str()) == parent && names_plural_of(path, &route.path));
    if let Some(route) = plural {
        let link = link_to(area, route);
        let note = format!(
            "back link to {} (`{}`): its path is the plural of the record's; `back` names \
             another page or `none`",
            link.href,
            handler_name(route)
        );
        return (Some(link), Some(note));
    }
    let note = format!(
        "no back link: area `{}` has no page listing `{}`; `back` names one",
        area.name, aggregate.name
    );
    (None, Some(note))
}

fn area_routes<'module>(module: &'module Module, area: &str) -> Vec<&'module Route> {
    module
        .areas
        .iter()
        .filter(|contribution| contribution.area == area)
        .flat_map(|contribution| &contribution.routes)
        .collect()
}

fn is_page(route: &Route) -> bool {
    route.method == Method::Get && path_parameters(&route.path).is_empty()
}

fn named_page<'route>(
    routes: &[&'route Route],
    area: &str,
    handler: &str,
) -> Result<&'route Route, GenError> {
    let named = routes
        .iter()
        .copied()
        .find(|route| is_page(route) && handler_name(route) == handler);
    named.ok_or_else(|| {
        let pages: Vec<&str> = routes
            .iter()
            .filter(|route| is_page(route))
            .map(|route| handler_name(route))
            .collect();
        GenError::Usage(format!(
            "`back` names a `get` page of area `{area}` without parameters, or `{NO_BACK_LINK}`; \
             no such page answers `{handler}`; its pages: {}",
            pages.join(", ")
        ))
    })
}

fn handler_name(route: &Route) -> &str {
    route.handler.last().map_or("", String::as_str)
}

/// The read model the route's handler answers, in any wrapper such as `Vec`, when it reads `aggregate`.
fn listed_model<'module>(
    module: &'module Module,
    route: &Route,
    aggregate: &Aggregate,
) -> Option<&'module ReadModel> {
    let code = &module.code;
    let output = code.function(&route.handler)?.output.as_ref()?;
    let mut types = vec![output];
    while let Some(ty) = types.pop() {
        let reads_aggregate = code.read_model(&ty.name).filter(|model| {
            model
                .from
                .as_deref()
                .and_then(|from| from.rsplit("::").next())
                == Some(aggregate.name.as_str())
        });
        if reads_aggregate.is_some() {
            return reads_aggregate;
        }
        types.extend(ty.arguments.iter());
    }
    None
}

fn parent_path(path: &str) -> Option<&str> {
    let (parent, last) = path.rsplit_once('/')?;
    let is_parameter = last.starts_with('{') && last.ends_with('}');
    match parent {
        _ if !is_parameter => None,
        "" => Some("/"),
        parent => Some(parent),
    }
}

/// `/orders` for `/orders/{order}`, `/categories` for `/categories/{category}`.
fn names_plural_of(record_path: &str, list_path: &str) -> bool {
    let Some(parameter) = path_parameters(record_path).last().copied() else {
        return false;
    };
    let listed = list_path.rsplit('/').next().unwrap_or_default();
    let plurals = [
        format!("{parameter}s"),
        format!("{parameter}es"),
        parameter
            .strip_suffix('y')
            .map(|stem| format!("{stem}ies"))
            .unwrap_or_default(),
    ];
    plurals.iter().any(|plural| plural == listed)
}

fn link_to(area: &KnownArea, route: &Route) -> BackLink {
    let prefix = area.prefix().unwrap_or_default();
    let href = if route.path == "/" && !prefix.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix}{}", route.path)
    };
    let named = route.screen.as_deref().unwrap_or(handler_name(route));
    let label = format!("Back to {}", sofie_engine::humanize(named).to_lowercase());
    BackLink { href, label }
}

#[cfg(test)]
mod tests {
    use super::{names_plural_of, parent_path};

    #[test]
    fn a_record_path_s_list_is_its_plural_one_level_up() {
        assert_eq!(parent_path("/orders/{order}"), Some("/orders"));
        assert_eq!(parent_path("/{order}"), Some("/"));
        assert_eq!(parent_path("/orders/{order}/lines"), None);
        assert!(names_plural_of("/orders/{order}", "/orders"));
        assert!(names_plural_of("/categories/{category}", "/categories"));
        assert!(names_plural_of("/boxes/{box}", "/boxes"));
        assert!(!names_plural_of("/orders/{order}", "/invoices"));
    }
}
