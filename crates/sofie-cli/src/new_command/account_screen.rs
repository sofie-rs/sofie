use std::path::PathBuf;

use sofie_model::App;

use super::Edits;
use crate::gen_command::GenError;
use crate::source_edits::{add_import, read};

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
        &[&area],
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
        &[&permitted, &member],
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
        &[&organization, &member],
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

fn mount(
    edits: &mut Edits,
    app: &App,
    module: &str,
    mounts: &[String],
    areas: &[&str],
) -> Result<(), GenError> {
    let library = app_library(app);
    edits.edit_rust(&library, |text| {
        let (imported, short_mounts) = shortened(text, mounts, areas);
        short_mounts
            .iter()
            .try_fold(imported, |text, mounted| mounted_as(&text, module, mounted))
            .ok_or_else(|| {
                let wanted: Vec<String> = mounts
                    .iter()
                    .map(|mounted| format!("`.area(\"{module}\", {mounted})`"))
                    .collect();
                GenError::Usage(format!(
                    "{} does not build the app with `wiring::app(…)`; add {} where it does",
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

/// The text importing the area's type, and the name to write it by; none when that name
/// already means something else here.
fn importable<'path>(text: &str, type_path: &'path str) -> Option<(String, &'path str)> {
    let (parent, name) = type_path.rsplit_once("::")?;
    let with_import = add_import(text, &format!("use {parent}"), name);
    let already_imported = with_import == text;
    if !already_imported && names(&text.replace(type_path, ""), name) {
        return None;
    }
    Some((with_import, name))
}

/// Each area named by its type alone where it can be imported.
fn shortened(text: &str, mounts: &[String], areas: &[&str]) -> (String, Vec<String>) {
    areas.iter().fold(
        (text.to_owned(), mounts.to_vec()),
        |(text, mounts), area| match importable(&text, area) {
            None => (text, mounts),
            Some((imported, name)) => {
                let mounts = mounts
                    .iter()
                    .map(|mounted| {
                        mounted
                            .replace(&format!("{area}>"), &format!("{name}>"))
                            .replace(&format!("{area},"), &format!("{name},"))
                    })
                    .collect();
                (imported, mounts)
            }
        },
    )
}

fn names(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(found, _)| {
        is_identifier_end(text[..found].chars().next_back())
            && is_identifier_end(text[found + name.len()..].chars().next())
    })
}

fn is_identifier_end(next: Option<char>) -> bool {
    next.is_none_or(|next| !(next.is_alphanumeric() || next == '_'))
}

/// The wiring's app gets one `.area(…)` more, the first turning it into a chain. Its
/// arguments vary: an app with flags passes them as well.
fn mounted_as(text: &str, module: &str, mounted: &str) -> Option<String> {
    let area = format!("\n        .area(\"{module}\", {mounted})");
    let start = text.find(WIRED)?;
    let end = start + text[start..].find(')')? + 1;
    if text[end..].starts_with('?') {
        let end = end + 1;
        return Some(format!("{}{area}{}", &text[..end], &text[end..]));
    }
    Some(format!(
        "{}Ok({}?{area}){}",
        &text[..start],
        &text[start..end],
        &text[end..]
    ))
}

const WIRED: &str = "wiring::app(";

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
    fn mounts_chain_on_the_wiring_s_app_given_flags_as_well() {
        let library = "pub async fn app(db: Db, flags: Flags) -> Result<App> {\n    wiring::app(db, flags)\n}\n";

        let once = super::mounted_as(
            library,
            "sofie_tenancy",
            "sofie_tenancy::member_routes::<A, B>",
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
            "pub async fn app(db: Db, flags: Flags) -> Result<App> {\n    Ok(wiring::app(db, flags)?\n        \
             .area(\"sofie_identity\", sofie_identity::passkeys::routes::<A>)\n        \
             .area(\"sofie_tenancy\", sofie_tenancy::member_routes::<A, B>))\n}\n"
        );
    }

    #[test]
    fn mounts_name_areas_by_an_imported_type() {
        let library = "use contract::area::Staff;\n\nfn app() {}\n";
        let mounts = ["routes::<contract::area::Member, contract::area::Staff>".to_owned()];

        let (text, mounts) = super::shortened(
            library,
            &mounts,
            &["contract::area::Member", "contract::area::Staff"],
        );

        assert_eq!(
            text,
            "use contract::area::{Member, Staff};\n\nfn app() {}\n"
        );
        assert_eq!(mounts, ["routes::<Member, Staff>"]);
    }

    #[test]
    fn mounts_keep_the_full_path_of_a_name_taken_by_another_type() {
        let library = "use other::Member;\n\nfn app() {}\n";
        let mounts = ["routes::<contract::area::Member>".to_owned()];

        let (text, mounts) = super::shortened(library, &mounts, &["contract::area::Member"]);

        assert_eq!(text, library);
        assert_eq!(mounts, ["routes::<contract::area::Member>"]);
    }

    #[test]
    fn each_name_reads_back_as_its_screen() {
        for screen in AccountScreen::ALL {
            assert_eq!(AccountScreen::parse(screen.name()), Some(screen));
        }
        assert_eq!(AccountScreen::parse("profile"), None);
    }
}
