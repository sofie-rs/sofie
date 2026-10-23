#![cfg(feature = "permitted-screens")]

//! The role screens render from `Role`'s and `RoleGrant`'s descriptions; the build script
//! compiles their template files for a client area instead, so the two must agree.

use sofie::db::Db;
use sofie::prelude::*;
use sofie_identity::{MemberArea, Memberships, Permitted, permitted_routes};

struct Console;

impl Area for Console {
    const NAME: &'static str = "console";
    const PREFIX: &'static str = "/console";
    const SIGN_IN: &'static str = "/console/sign-in";
    type Principal = Permitted;
}

impl MemberArea for Console {
    const HOME: &'static str = "/console/roles";
    const SIGN_UP: Option<&'static str> = None;
}

/// Each file with `{prefix}` where its area's prefix goes.
const TEMPLATES: [(&str, &str); 6] = [
    (
        "role_index",
        include_str!("../src/permitted/screens/role_index.html"),
    ),
    (
        "role_show",
        include_str!("../src/permitted/screens/role_show.html"),
    ),
    (
        "role_create",
        include_str!("../src/permitted/screens/role_create.html"),
    ),
    (
        "role_edit",
        include_str!("../src/permitted/screens/role_edit.html"),
    ),
    (
        "role_grant_index",
        include_str!("../src/permitted/screens/role_grant_index.html"),
    ),
    (
        "role_grant_create",
        include_str!("../src/permitted/screens/role_grant_create.html"),
    ),
];

#[tokio::test]
async fn the_role_templates_hold_the_source_their_resources_render() {
    sofie_testing::start(
        |db: Db| async move {
            db.schema().migrate(&[sofie_identity::migrations()]).await?;
            let mut screens = sofie::http::Screens::default();
            screens.add_builtin(sofie_identity::module(&[]).screens)?;
            let app = App::new(db)
                .screens(screens)
                .membership(Memberships)
                .area::<Web>("sofie_identity", sofie_identity::routes::<Console>)
                .area::<Console>("sofie_identity", permitted_routes::<Console>);
            for (name, template) in TEMPLATES {
                let key = format!("sofie_identity/console/{name}");
                let rendered = sofie::http::built_in_screen_source(&app, &key);
                let expected = template.replace("{prefix}", Console::PREFIX);
                assert_eq!(rendered.as_deref(), Some(expected.as_str()), "{key}");
            }
            Ok(app)
        },
        sofie::worker::Worker::new,
    )
    .await;
}
