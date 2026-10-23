use std::fs;

use sofie_cli::{AccountScreen, NewCode, new_code};
use sofie_model::testing::{TemporaryDirectory, manifest};

fn shop() -> TemporaryDirectory {
    let directory = TemporaryDirectory::new("sofie-new-mail");
    directory.write(
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"modules/*\", \"modules/*/contract\"]\n\n\
         [workspace.dependencies]\nsofie = { path = \"sofie\" }\n\
         sofie-identity = { path = \"sofie-identity\" }\n",
    );
    directory.write("sofie/Cargo.toml", &manifest("sofie", ""));
    directory.write("sofie/src/lib.rs", "");
    directory.write(
        "sofie-identity/Cargo.toml",
        &format!(
            "{}\n[features]\nsession-screens = []\ntotp = []\n",
            manifest("sofie-identity", "")
        ),
    );
    directory.write("sofie-identity/src/lib.rs", "");
    directory.write(
        "app/Cargo.toml",
        &manifest(
            "shop",
            "members = { path = \"../modules/members\" }\n\
             members-contract = { path = \"../modules/members/contract\" }\n",
        ),
    );
    directory.write("app/src/main.rs", "fn main() {}\n");
    directory.write(
        "app/src/lib.rs",
        "mod wiring;\n\npub async fn app(db: Db) -> Result<App> {\n    wiring::app(db)\n}\n",
    );
    directory.write(
        "modules/members/contract/Cargo.toml",
        &manifest("members-contract", ""),
    );
    directory.write("modules/members/contract/src/lib.rs", "pub mod area;\n");
    directory.write(
        "modules/members/contract/src/area.rs",
        "pub struct Member;\n\nimpl Area for Member {\n    const NAME: &'static str = \"member\";\n    \
         const PREFIX: &'static str = \"/account\";\n    const SIGN_IN: &'static str = \"/sign-in\";\n    \
         type Principal = sofie_identity::Account;\n}\n\n\
         pub struct Staff;\n\nimpl Area for Staff {\n    const NAME: &'static str = \"staff\";\n    \
         const PREFIX: &'static str = \"/staff\";\n    const SIGN_IN: &'static str = \"/staff/sign-in\";\n    \
         type Principal = sofie_identity::Permitted;\n}\n",
    );
    directory.write("modules/members/Cargo.toml", &manifest("members", ""));
    directory.write(
        "modules/members/src/lib.rs",
        "pub mod module;\n\npub mod areas {\n    pub mod web {\n        pub mod routes;\n    }\n}\n",
    );
    directory.write(
        "modules/members/src/module.rs",
        "pub const MODULE: sofie::Module = sofie::Module {\n    name: \"members\",\n    tier: sofie::Tier::Core,\n    context: None,\n    settings: &[],\n};\n",
    );
    directory.write(
        "modules/members/src/areas/web/routes.rs",
        "use sofie::prelude::*;\n\npub fn routes(_routes: &mut Routes<Web>) {}\n",
    );
    directory
}

fn read(directory: &TemporaryDirectory, relative: &str) -> String {
    fs::read_to_string(directory.path(relative)).unwrap()
}

fn generate(directory: &TemporaryDirectory, code: &NewCode) -> Vec<String> {
    let root = format!("{}/", directory.root().display());
    new_code(directory.root(), code)
        .unwrap()
        .iter()
        .map(|change| change.to_string().replacen(&root, "", 1))
        .collect()
}

fn mail(name: &str, list: Option<&str>) -> NewCode {
    NewCode::Mail {
        module: "members".to_owned(),
        name: name.to_owned(),
        list: list.map(str::to_owned),
    }
}

fn screen(screen: &str) -> NewCode {
    NewCode::AccountScreen {
        area: "member".to_owned(),
        screen: AccountScreen::parse(screen).unwrap(),
    }
}

#[test]
fn a_new_mail_writes_its_template_and_the_function_sending_it() {
    let shop = shop();

    let changes = generate(&shop, &mail("weekly_news", Some("news")));
    generate(&shop, &mail("welcome", None));

    assert!(
        changes.contains(&"created modules/members/src/mails/weekly_news.html".to_owned()),
        "{changes:?}"
    );
    let sending = read(&shop, "modules/members/src/mails.rs");
    assert!(
        sending.contains("mailer.body(\"members/weekly_news\""),
        "{sending}"
    );
    assert!(sending.contains(".bulk(\"news\")?"), "{sending}");
    assert_eq!(
        sending
            .matches("use sofie::mail::{Address, Mail, Mailer};\n")
            .count(),
        1,
        "the second mail joins the imports the first wrote: {sending}"
    );
    assert!(
        sending.contains("pub fn welcome(mailer: &Mailer, to: Address)"),
        "{sending}"
    );
    assert!(read(&shop, "modules/members/src/lib.rs").contains("mod mails;"));
    let cargo = read(&shop, "modules/members/Cargo.toml");
    assert!(
        cargo.contains("\"mail-web\"") && cargo.contains("\"mail\""),
        "{cargo}"
    );
    assert!(
        new_code(shop.root(), &mail("welcome", None)).is_err(),
        "a name is taken once"
    );
    assert!(new_code(shop.root(), &mail("offers", Some("Big Offers"))).is_err());
}

#[test]
fn an_account_screen_is_mounted_for_its_area_with_its_identity_feature() {
    let shop = shop();

    generate(&shop, &screen("sessions"));
    generate(&shop, &screen("two-factor"));

    let library = read(&shop, "app/src/lib.rs");
    assert!(library.contains("Ok(wiring::app(db)?"), "{library}");
    assert!(
        library.contains("sofie_identity::sessions::routes::<members_contract::area::Member>"),
        "{library}"
    );
    assert!(
        library.contains("sofie_identity::two_factor::routes::<"),
        "{library}"
    );
    let cargo = read(&shop, "app/Cargo.toml");
    assert!(
        cargo.contains(
            "sofie-identity = { workspace = true, features = [\"session-screens\", \"totp\"] }"
        ),
        "{cargo}"
    );
    assert!(
        new_code(shop.root(), &screen("sessions")).is_err(),
        "mounted once"
    );
}

#[test]
fn impersonation_mounts_both_halves_with_its_feature() {
    let shop = shop();

    generate(
        &shop,
        &NewCode::Impersonation {
            permitted: "staff".to_owned(),
            member: "member".to_owned(),
        },
    );

    let library: String = read(&shop, "app/src/lib.rs").split_whitespace().collect();
    assert!(
        library.contains("sofie_identity::impersonation::routes::<members_contract::area::Staff,members_contract::area::Member,>"),
        "{library}"
    );
    assert!(
        library.contains("sofie_identity::impersonation::stop_routes::<members_contract::area::Member,members_contract::area::Staff,>"),
        "{library}"
    );
    assert!(read(&shop, "app/Cargo.toml").contains("features = [\"impersonation\"]"));
}

#[test]
fn tenancy_mounts_both_areas_and_depends_on_sofie_tenancy() {
    let shop = shop();

    generate(
        &shop,
        &NewCode::Tenancy {
            organization: "staff".to_owned(),
            member: "member".to_owned(),
        },
    );

    let library: String = read(&shop, "app/src/lib.rs").split_whitespace().collect();
    assert!(
        library.contains("sofie_tenancy::member_routes::<members_contract::area::Member,members_contract::area::Staff,>"),
        "{library}"
    );
    assert!(
        library.contains("sofie_tenancy::organization_routes::<members_contract::area::Staff,members_contract::area::Member,>"),
        "{library}"
    );
    assert!(read(&shop, "app/Cargo.toml").contains("sofie-tenancy.workspace = true"));
    let workspace = read(&shop, "Cargo.toml");
    assert!(
        workspace.contains(
            "sofie-identity = { path = \"sofie-identity\" }\nsofie-tenancy = { path = \""
        ),
        "the workspace declares what the app now inherits from it, as `sofie new app` declares sofie: {workspace}"
    );
    assert!(workspace.contains("/sofie-tenancy\" }\n"), "{workspace}");
}
