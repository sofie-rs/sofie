use sofie_model::naming::{RouteSite, path_parameters, screen_key, snake_case};
use sofie_model::{KnownArea, Method};

use super::facts::{Fact, ModuleFacts};
use super::requests::{Requests, caller};
use super::route_facts::handler;

/// A list screen whose `Table` links each row to a page of the module's area: one record listed
/// for the member links to that record's page.
pub(super) fn row_links(
    requests: &Requests,
    site: &RouteSite,
    area: &KnownArea,
    screen: &str,
    facts: &mut ModuleFacts,
) {
    if !path_parameters(&site.route.path).is_empty() {
        return;
    }
    let links = match table_links(requests, site, screen) {
        Ok(links) => links,
        Err(reason) => {
            let name = site.name();
            facts.uncover(&name, format!("{name}: {reason}"));
            return;
        }
    };
    let prefix = area.prefix().unwrap_or_default();
    for link in links {
        let linked = requests.app.route_sites().into_iter().find(|candidate| {
            std::ptr::eq(candidate.module, requests.module)
                && candidate.area.area == site.area.area
                && candidate.route.method == Method::Get
                && written_as_link(prefix, &candidate.route.path) == link
        });
        if let Some(fact) = linked.and_then(|linked| row_link(requests, site, &linked, area, &link))
        {
            facts.facts.push(fact);
        }
    }
}

/// A screen that cannot be read or parsed has no links to test; `sofie check` reports it.
fn table_links(requests: &Requests, site: &RouteSite, screen: &str) -> Result<Vec<String>, String> {
    let file = site.area.screen_file(screen);
    if !site.area.screens.contains(&file) {
        return Ok(Vec::new());
    }
    let read = requests
        .screens
        .screen(&file)
        .as_ref()
        .map_err(|error| format!("its screen {} cannot be read: {error}", file.display()))?;
    let name = file.display().to_string();
    sofie_engine::table_links_with(&name, &read.source, requests.screens.components())
        .map(|links| links.into_iter().map(|table| table.link).collect())
        .map_err(|error| format!("its row links are not tested, as its screen is refused: {error}"))
}

fn row_link(
    requests: &Requests,
    list: &RouteSite,
    linked: &RouteSite,
    area: &KnownArea,
    link: &str,
) -> Option<Fact> {
    let [parameter] = path_parameters(&linked.route.path)[..] else {
        return None;
    };
    let member = requests.member(&[list, linked], area)?;
    let made = requests.make_parameter(linked, parameter, "member", "record")?;
    let request = requests.request(list, &[])?;
    let screen = list.route.screen.as_deref()?;
    let key = screen_key(requests.crate_name(), &list.area.area, screen);
    let href = format!("href=\"{}\"", link.replace("{}", "{record}"));
    Some(Fact {
        subject: list.name(),
        text: format!("screen {key} links a row to route {}", linked.name()),
        test: format!(
            "{}_screen_links_a_row_to_{}",
            snake_case(screen),
            handler(linked)
        ),
        body: format!(
            "{member}{made}let page = {}.request(&{request}).header(\"accept\", \"text/html\").await;\npage.assert_ok();\nlet href = format!({href:?});\nassert!(page.body.contains(&href), \"{{href}} missing from {{}}\", page.body);",
            caller(area)
        ),
        on_app: true,
    })
}

/// A route path as a `Table` link to it reads once its bound parts are `{}`.
fn written_as_link(prefix: &str, path: &str) -> String {
    let mut written = prefix.to_owned();
    let mut rest = path;
    while let Some((before, after)) = rest.split_once('{') {
        written.push_str(before);
        written.push_str("{}");
        rest = after.split_once('}').map_or("", |(_, after)| after);
    }
    written.push_str(rest);
    written
}
