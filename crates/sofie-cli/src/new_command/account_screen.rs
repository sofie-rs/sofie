use std::path::PathBuf;

use sofie_model::App;

use super::Edits;
use crate::gen_command::GenError;
use crate::source_edits::read;

/// A member's own screen sofie-identity ships, mounted in one of the module's areas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountScreen {
    Sessions,
    TwoFactor,
    Passkeys,
    Connections,
    AccountData,
}

impl AccountScreen {
    pub const ALL: [Self; 5] = [
        Self::Sessions,
        Self::TwoFactor,
        Self::Passkeys,
        Self::Connections,
        Self::AccountData,
    ];

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|screen| screen.name() == name)
    }

    /// Its name on the command line, the sofie-identity feature that brings it, and the
    /// sofie-identity module mounting it.
    fn parts(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Sessions => ("sessions", "session-screens", "sessions"),
            Self::TwoFactor => ("two-factor", "totp", "two_factor"),
            Self::Passkeys => ("passkeys", "passkeys", "passkeys"),
            Self::Connections => ("connections", "oauth", "oauth"),
            Self::AccountData => ("account-data", "account-data", "account_data"),
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        self.parts().0
    }

    fn mount(self) -> String {
        format!("{IDENTITY}::{}::routes", self.parts().2)
    }
}

/// The app's `app` mounts it for the area, as `sofie routes` reads it, and the app's
/// sofie-identity dependency gets its feature.
pub(super) fn new_account_screen(
    edits: &mut Edits,
    app: &App,
    area: &str,
    screen: AccountScreen,
) -> Result<(), GenError> {
    let area = defined_area(app, area)?;
    refuse_mounted(app, &screen.mount())?;
    edits.enable_feature(&app.app, "sofie-identity", screen.parts().1)?;
    mount(
        edits,
        app,
        IDENTITY,
        &[format!("{}::<{area}>", screen.mount())],
    )
}

/// Signing in as a member of `member` for a permitted principal of `permitted`, each area
/// mounting its half.
pub(super) fn new_impersonation(
    edits: &mut Edits,
    app: &App,
    permitted: &str,
    member: &str,
) -> Result<(), GenError> {
    let permitted = defined_area(app, permitted)?;
    let member = defined_area(app, member)?;
    refuse_mounted(app, IMPERSONATION)?;
    edits.enable_feature(&app.app, "sofie-identity", "impersonation")?;
    mount(
        edits,
        app,
        IDENTITY,
        &[
            format!("{IMPERSONATION}::routes::<{permitted}, {member}>"),
            format!("{IMPERSONATION}::stop_routes::<{member}, {permitted}>"),
        ],
    )
}

const IDENTITY: &str = "sofie_identity";
const IMPERSONATION: &str = "sofie_identity::impersonation";

/// Organizations with their members, where `organization` acts for one and `member` holds
/// an account's list of them; both mounted, and the app depending on sofie-tenancy.
pub(super) fn new_tenancy(
    edits: &mut Edits,
    app: &App,
    organization: &str,
    member: &str,
) -> Result<(), GenError> {
    let organization = defined_area(app, organization)?;
    let member = defined_area(app, member)?;
    refuse_mounted(app, TENANCY)?;
    edits.depend_on_workspace(&app.app, "sofie-tenancy")?;
    mount(
        edits,
        app,
        TENANCY,
        &[
            format!("{TENANCY}::member_routes::<{member}, {organization}>"),
            format!("{TENANCY}::organization_routes::<{organization}, {member}>"),
        ],
    )
}

const TENANCY: &str = "sofie_tenancy";

fn app_library(app: &App) -> PathBuf {
    app.app.path.join("src").join("lib.rs")
}

fn refuse_mounted(app: &App, mounted: &str) -> Result<(), GenError> {
    let library = app_library(app);
    if read(&library)?.contains(mounted) {
        return Err(GenError::Usage(format!(
            "{} mounts `{mounted}` already",
            library.display()
        )));
    }
    Ok(())
}

fn mount(edits: &mut Edits, app: &App, module: &str, mounts: &[String]) -> Result<(), GenError> {
    let library = app_library(app);
    edits.edit_rust(&library, |text| {
        mounts
            .iter()
            .try_fold(text.to_owned(), |text, mounted| {
                mounted_as(&text, module, mounted)
            })
            .ok_or_else(|| {
                let wanted: Vec<String> = mounts
                    .iter()
                    .map(|mounted| format!("`.area(\"{module}\", {mounted})`"))
                    .collect();
                GenError::Usage(format!(
                    "{} does not build the app with `wiring::app(db)`; add {} where it does",
                    library.display(),
                    wanted.join(" and ")
                ))
            })
    })
}

fn defined_area(app: &App, name: &str) -> Result<String, GenError> {
    app.areas()
        .into_iter()
        .find(|known| known.name == name && known.definition.is_some())
        .map(|known| known.type_path)
        .ok_or_else(|| GenError::Usage(format!("the app defines no area `{name}`")))
}

/// The wiring's app gets one `.area(…)` more, the first turning it into a chain.
fn mounted_as(text: &str, module: &str, mounted: &str) -> Option<String> {
    let area = format!("\n        .area(\"{module}\", {mounted})");
    if let Some(at) = text.find(CHAINED) {
        let end = at + CHAINED.len();
        return Some(format!("{}{area}{}", &text[..end], &text[end..]));
    }
    let at = text.find(WIRED)?;
    let end = at + WIRED.len();
    Some(format!(
        "{}Ok({CHAINED}{area}){}",
        &text[..at],
        &text[end..]
    ))
}

const WIRED: &str = "wiring::app(db)";
const CHAINED: &str = "wiring::app(db)?";

#[cfg(test)]
mod tests {
    use super::AccountScreen;

    #[test]
    fn mounts_chain_on_the_wiring_s_app() {
        let library = "pub async fn app(db: Db) -> Result<App> {\n    wiring::app(db)\n}\n";

        let once = super::mounted_as(
            library,
            "sofie_identity",
            "sofie_identity::sessions::routes::<A>",
        )
        .unwrap();
        let twice = super::mounted_as(
            &once,
            "sofie_identity",
            "sofie_identity::passkeys::routes::<A>",
        )
        .unwrap();

        assert_eq!(
            twice,
            "pub async fn app(db: Db) -> Result<App> {\n    Ok(wiring::app(db)?\n        \
             .area(\"sofie_identity\", sofie_identity::passkeys::routes::<A>)\n        \
             .area(\"sofie_identity\", sofie_identity::sessions::routes::<A>))\n}\n"
        );
    }

    #[test]
    fn each_name_reads_back_as_its_screen() {
        for screen in AccountScreen::ALL {
            assert_eq!(AccountScreen::parse(screen.name()), Some(screen));
        }
        assert_eq!(AccountScreen::parse("profile"), None);
    }
}
