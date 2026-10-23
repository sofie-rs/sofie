use sofie_model::{App, AreaClient};

use super::manifest::{declaration_line, manifest_text};
use super::{Finding, Rule};

const FRAMEWORK_PACKAGE: &str = "sofie-framework";
const CLIENTS_FEATURE: &str = "clients";

/// The wiring hands each client area its bundle through sofie's `clients` feature, so without
/// it the app fails to compile on a name the error does not explain.
#[must_use]
pub fn check_client_feature(app: &App, framework_features: &[String]) -> Vec<Finding> {
    if framework_features
        .iter()
        .any(|feature| feature == CLIENTS_FEATURE)
    {
        return Vec::new();
    }
    let areas = app.areas();
    let Some(area) = areas.iter().find(|area| {
        area.definition
            .is_some_and(|definition| definition.client != AreaClient::Html)
    }) else {
        return Vec::new();
    };
    let (manifest, text) = manifest_text(&app.app);
    let line = app
        .app
        .dependencies
        .iter()
        .find(|dependency| dependency.package == FRAMEWORK_PACKAGE)
        .map_or(1, |dependency| {
            declaration_line(&text, &dependency.crate_name)
        });
    vec![Finding {
        rule: Rule::ClientFeature,
        file: manifest,
        line,
        message: format!(
            "area `{}` names a client, but the app does not enable sofie's `{CLIENTS_FEATURE}` \
             feature, which serves client areas",
            area.name
        ),
        fix: format!(
            "add \"{CLIENTS_FEATURE}\" to the `features` of the app's `sofie` dependency in \
             app/Cargo.toml"
        ),
    }]
}
