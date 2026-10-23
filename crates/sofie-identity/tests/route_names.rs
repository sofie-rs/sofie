//! A binary of its own: a span's interest is cached per callsite.
#![cfg(all(feature = "password-reset", feature = "email-verification"))]

use sofie::db::Db;
use sofie::http::{App, Area, ListedRoute, Screens, Web};
use sofie_identity::{Account, MemberArea, Memberships};
use sofie_testing::{FakeDriver, TestServer};

struct Members;

impl Area for Members {
    const NAME: &'static str = "members";
    const PREFIX: &'static str = "/members";
    const SIGN_IN: &'static str = "/members/sign-in";
    type Principal = Account;
}

impl MemberArea for Members {
    const HOME: &'static str = "/members/home";
    const SIGN_UP: Option<&'static str> = Some("/members/sign-up");
}

/// Every area-generic route the enabled features bring, beside the member routes.
struct Console;

impl Area for Console {
    const NAME: &'static str = "console";
    const PREFIX: &'static str = "/console";
    const SIGN_IN: &'static str = "/console/sign-in";
    #[cfg(feature = "tokens")]
    const TOKENS: bool = true;
    #[cfg(feature = "permissions")]
    type Principal = sofie_identity::Permitted;
    #[cfg(not(feature = "permissions"))]
    type Principal = Account;
}

fn app() -> App {
    let mut screens = Screens::default();
    screens
        .add_builtin(sofie_identity::module(&[]).screens)
        .unwrap();
    screens.describe_forms(sofie_identity::FORMS).unwrap();
    App::new(Db::new(FakeDriver::default()))
        .screens(screens)
        .membership(Memberships)
        .area::<Web>("sofie_identity", sofie_identity::routes::<Members>)
        .area::<Console>("sofie_identity", |routes| {
            #[cfg(feature = "permitted-screens")]
            sofie_identity::permitted_routes(routes);
            #[cfg(feature = "token-screens")]
            sofie_identity::tokens::screen::routes(routes);
            #[cfg(feature = "impersonation")]
            sofie_identity::impersonation::routes::<Console, Members>(routes);
            let _ = routes;
        })
        .area::<Members>("sofie_identity", |routes| {
            #[cfg(feature = "sessions")]
            sofie_identity::sessions::routes(routes);
            #[cfg(feature = "totp")]
            sofie_identity::two_factor::routes(routes);
            #[cfg(feature = "passkeys")]
            sofie_identity::passkeys::routes(routes);
            #[cfg(feature = "oauth")]
            sofie_identity::oauth::routes(routes);
            #[cfg(feature = "account-data")]
            sofie_identity::account_data::routes(routes);
            #[cfg(feature = "impersonation")]
            sofie_identity::impersonation::stop_routes::<Members, Console>(routes);
            let _ = routes;
        })
}

/// A member's own screens, mounted where members sign in rather than in a `Permitted` area.
const MEMBER_MOUNTS: [&str; 6] = [
    "sessions::routes",
    "two_factor::routes",
    "passkeys::routes",
    "oauth::routes",
    "account_data::routes",
    "impersonation::stop_routes",
];

fn requested(listed: &ListedRoute) -> (String, String) {
    let id = "0190a3b2-6c1d-7e2f-8a3b-4c5d6e7f8091";
    let (area, path) = if listed.mount == "routes" {
        let path = listed
            .path
            .replace("{sign_in}", Members::SIGN_IN)
            .replace("{sign_up}", Members::SIGN_UP.unwrap_or_default())
            .replace("{prefix}", Members::PREFIX);
        (Members::NAME, path)
    } else if MEMBER_MOUNTS.contains(&listed.mount) {
        (Members::NAME, format!("{}{}", Members::PREFIX, listed.path))
    } else {
        (Console::NAME, format!("{}{}", Console::PREFIX, listed.path))
    };
    let path = [
        "{account_identity}",
        "{account_link}",
        "{account_session}",
        "{passkey}",
        "{role}",
        "{role_grant}",
    ]
    .into_iter()
    .fold(path, |path, parameter| path.replace(parameter, id));
    let name = format!("{} sofie_identity.{area}.{}", listed.method, listed.handler);
    (path, name)
}

#[tokio::test]
async fn every_listed_route_is_named_module_area_handler() {
    let telemetry = sofie::telemetry::capture();
    let server = TestServer::start(app()).await;
    let mut expected = Vec::new();

    for listed in sofie_identity::ROUTES {
        let (path, name) = requested(listed);
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
