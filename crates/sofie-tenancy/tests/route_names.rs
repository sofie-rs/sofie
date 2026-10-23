//! A binary of its own: a span's interest is cached per callsite.

use sofie::db::Db;
use sofie::http::{App, Area, ListedRoute, Screens, Web};
use sofie_identity::{Account, MemberArea, Memberships};
use sofie_tenancy::{Organization, OrganizationArea};
use sofie_testing::{FakeDriver, TestServer};

struct Member;

impl Area for Member {
    const NAME: &'static str = "member";
    const PREFIX: &'static str = "/account";
    const SIGN_IN: &'static str = "/sign-in";
    type Principal = Account;
}

impl MemberArea for Member {
    const HOME: &'static str = "/account/organizations";
    const SIGN_UP: Option<&'static str> = Some("/sign-up");
}

struct Workspace;

impl Area for Workspace {
    const NAME: &'static str = "workspace";
    const PREFIX: &'static str = "/org";
    const SIGN_IN: &'static str = "/account/organizations";
    type Principal = Organization;
}

impl OrganizationArea for Workspace {
    const HOME: &'static str = "/org/members";
}

fn app() -> App {
    let mut screens = Screens::default();
    screens
        .add_builtin(sofie_identity::module(&[]).screens)
        .unwrap();
    App::new(Db::new(FakeDriver::default()))
        .screens(screens)
        .membership(Memberships)
        .area::<Web>("sofie_identity", sofie_identity::routes::<Member>)
        .area::<Member>(
            "sofie_tenancy",
            sofie_tenancy::member_routes::<Member, Workspace>,
        )
        .area::<Workspace>(
            "sofie_tenancy",
            sofie_tenancy::organization_routes::<Workspace, Member>,
        )
}

fn requested<A: Area>(listed: &ListedRoute) -> (String, String) {
    let id = "0190a3b2-6c1d-7e2f-8a3b-4c5d6e7f8091";
    let path = ["{organization_member}", "{invitation}"].into_iter().fold(
        format!("{}{}", A::PREFIX, listed.path),
        |path, parameter| path.replace(parameter, id),
    );
    let name = format!(
        "{} sofie_tenancy.{}.{}",
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
    let requests = sofie_tenancy::MEMBER_ROUTES
        .iter()
        .map(|listed| (listed, requested::<Member>(listed)))
        .chain(
            sofie_tenancy::ORGANIZATION_ROUTES
                .iter()
                .map(|listed| (listed, requested::<Workspace>(listed))),
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
