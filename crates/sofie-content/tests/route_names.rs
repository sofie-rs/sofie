//! A binary of its own: a span's interest is cached per callsite.

use sofie::db::Db;
use sofie::http::{App, Area, ListedRoute, Screens, Visitor, Web};
use sofie_testing::{FakeDriver, TestServer};

struct Desk;

impl Area for Desk {
    const NAME: &'static str = "desk";
    const PREFIX: &'static str = "/desk";
    const SIGN_IN: &'static str = "/";
    type Principal = Visitor;
}

fn app() -> App {
    let mut screens = Screens::default();
    screens.add_builtin(sofie_content::MODULE.screens).unwrap();
    App::new(Db::new(FakeDriver::default()))
        .screens(screens)
        .area::<Web>("sofie_content", sofie_content::routes)
        .area::<Desk>("sofie_content", sofie_content::admin_routes::<Desk>)
}

fn requested<A: Area>(listed: &ListedRoute) -> (String, String) {
    let path = format!("{}{}", A::PREFIX, listed.path)
        .replace("{slug}", "about")
        .replace("{page}", "0190a3b2-6c1d-7e2f-8a3b-4c5d6e7f8091");
    let name = format!(
        "{} sofie_content.{}.{}",
        listed.method,
        A::NAME,
        listed.handler
    );
    (path, name)
}

#[tokio::test]
async fn every_listed_route_is_named_module_area_handler() {
    let telemetry = sofie::telemetry::capture();
    let server = TestServer::start(app()).await;
    let requests = sofie_content::ROUTES
        .iter()
        .map(|listed| (listed, requested::<Web>(listed)))
        .chain(
            sofie_content::ADMIN_ROUTES
                .iter()
                .map(|listed| (listed, requested::<Desk>(listed))),
        );
    let mut expected = Vec::new();

    for (listed, (path, name)) in requests {
        match listed.method {
            "GET" => server.get(&path).await,
            _ => server.post(&path).await,
        };
        expected.push(name);
    }

    let span_names: Vec<String> = telemetry
        .spans()
        .into_iter()
        .filter(|span| span.name == "request")
        .filter_map(|span| span.fields.get("otel.name").cloned())
        .collect();
    assert_eq!(span_names, expected);
}
