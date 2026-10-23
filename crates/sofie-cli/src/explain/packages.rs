use std::path::PathBuf;

use sofie_model::naming::{IDENTITY_AREA, IDENTITY_PACKAGE, crate_name};
use sofie_model::{App, Identity, ModelError};

/// An app renders sofie-identity's screens under its own keys, as its start-up errors name them.
pub(super) fn identity_screen(app: &App, name: &str) -> Option<String> {
    let identity = app.identity.as_ref()?;
    let prefix = format!("{}/", crate_name(IDENTITY_PACKAGE));
    let (area, stem) = name.strip_prefix(&prefix)?.split_once('/')?;
    let (template, handlers) =
        if area == IDENTITY_AREA && identity.screens.iter().any(|key| key == name) {
            (
                member_template(identity, stem),
                member_handlers(identity, stem),
            )
        } else {
            let known = app.area(area)?;
            let mounted = identity
                .mounted_screens(&known)
                .into_iter()
                .find(|mounted| mounted.key == name)?;
            (
                mounted.screen.file.clone(),
                Ok(vec![mounted.screen.handler.clone()]),
            )
        };
    let mut lines = vec![
        format!("screen {name}"),
        format!("  package: {IDENTITY_PACKAGE}, area {area}"),
        format!("  template: {}", template.display()),
    ];
    lines.extend(shown_by(app, area, handlers));
    Some(lines.join("\n"))
}

fn member_template(identity: &Identity, stem: &str) -> PathBuf {
    identity
        .directory
        .join("src")
        .join("screens")
        .join(format!("{stem}.html"))
}

fn member_handlers(identity: &Identity, stem: &str) -> Result<Vec<String>, ModelError> {
    Ok(identity
        .screen_routes()?
        .into_iter()
        .filter(|route| route.screen == stem)
        .map(|route| route.handler)
        .collect())
}

fn shown_by(app: &App, area: &str, handlers: Result<Vec<String>, ModelError>) -> Vec<String> {
    let routes = handlers.and_then(|handlers| {
        let served = app.package_routes().identity()?;
        Ok(served
            .into_iter()
            .filter(|served| served.area == area && handlers.contains(&served.route.handler))
            .map(|served| {
                format!(
                    "  shown by: route {} ({} {})",
                    served.name, served.route.method, served.path
                )
            })
            .collect::<Vec<String>>())
    });
    match routes {
        Ok(lines) if lines.is_empty() => vec![format!(
            "  shown by: no route naming it by a literal in {IDENTITY_PACKAGE}'s code; `routes` \
             lists the package's routes"
        )],
        Ok(lines) => lines,
        Err(error) => vec![format!(
            "  shown by: {IDENTITY_PACKAGE}'s routes could not be read: {error}"
        )],
    }
}
