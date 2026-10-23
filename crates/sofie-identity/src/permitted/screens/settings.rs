use sofie::flags::{Features, Flags};
use sofie::http::{Area, NeedsPermission, Permission, Routes, Shipped};
use sofie::prelude::*;
use sofie::serde_json::{Value, json};

use super::Listed;

const READ: Permission = Permission::new("sofie_identity.setting.*.read");
const UPDATE: Permission = Permission::new("sofie_identity.setting.*.update");

pub(super) fn routes<A: Area>(routes: &mut Routes<A>) {
    let describe: sofie::http::Describe =
        |screens, key| screens.describe_form::<Change>(key, "change");
    routes
        .shipped(
            "/settings",
            "settings",
            (include_str!("settings.html"), Some(describe)),
            list,
        )
        .name("list_settings")
        .permission(READ);
    routes
        .post("/settings", change)
        .name("change_setting")
        .permission(UPDATE)
        .then_screen("settings", "settings");
}

async fn list(features: Features) -> Result<Listed> {
    let rows = features
        .settings()
        .into_iter()
        .map(|setting| {
            json!({
                "name": setting.name,
                "value": setting.value.to_string(),
                "default": setting.default.to_string(),
            })
        })
        .collect();
    Ok(Listed(Value::Array(rows)))
}

#[derive(Input)]
struct Change {
    #[required]
    setting: String,
    value: String,
}

async fn change(flags: Flags, change: Change) -> Result<()> {
    flags.change_setting(&change.setting, &change.value).await
}
