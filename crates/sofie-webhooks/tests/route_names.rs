//! A binary of its own: a span's interest is cached per callsite.

use sofie::db::Db;
use sofie::http::{Access, App, Area, AreaPrincipal, ListedRoute, Method};
use sofie_testing::{FakeDriver, TestServer};
use sofie_webhooks::{WebhookArea, WebhookEvent};

struct Customer;

impl AreaPrincipal for Customer {
    const ACCESS: Access = Access::Owned;
    type Record = Self;
}

struct Member;

impl Area for Member {
    const NAME: &'static str = "member";
    const PREFIX: &'static str = "/account";
    const SIGN_IN: &'static str = "/sign-in";
    type Principal = Customer;
}

impl WebhookArea for Member {
    const WEBHOOK_EVENTS: &'static [WebhookEvent] = &[];
}

fn requested(listed: &ListedRoute) -> (Method, String, String) {
    let id = "0190a3b2-6c1d-7e2f-8a3b-4c5d6e7f8091";
    let path = ["{webhook_endpoint}", "{webhook_delivery}"]
        .into_iter()
        .fold(
            format!("{}{}", Member::PREFIX, listed.path),
            |path, parameter| path.replace(parameter, id),
        );
    let method = Method::from_bytes(listed.method.as_bytes()).unwrap();
    let name = format!("{} sofie_webhooks.member.{}", listed.method, listed.handler);
    (method, path, name)
}

#[tokio::test]
async fn every_listed_route_is_named_module_area_handler() {
    let telemetry = sofie::telemetry::capture();
    let app = App::new(Db::new(FakeDriver::default()))
        .area::<Member>("sofie_webhooks", sofie_webhooks::routes::<Member>);
    let server = TestServer::start(app).await;
    let mut expected = Vec::new();

    for listed in sofie_webhooks::ROUTES {
        let (method, path, name) = requested(listed);
        server.request(method, &path).await;
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
